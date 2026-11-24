//! Durable delegation endpoints. The supervisor runs persisted work from the
//! store.

mod delivery;
mod drive;
pub(crate) mod store;

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::{Mutex as AsyncMutex, Notify, broadcast},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use viz_core::tasks::{self as decisions, Command, Delegator, Rejection, is_settled};
use viz_protocol::{
    CancelTask, ClearTasks, Cleared, DelegateInput, DelegateTask, DeleteTask, Empty, ListTasks,
    TaskId, TaskList, TaskResult, TaskStatusRequest, TaskStatusResult,
    peer::{ReceiveTask, TaskTarget},
};

use self::store::{New, Stored, prior_request, stop_exists};
use crate::{
    environments::Peers,
    error::Error,
    rpc::handlers,
    wire::{RunId, Task, ThreadId},
    workspace::{Workspace, active_run_from, sql},
};

const POLL: Duration = Duration::from_secs(1);
const LOCAL_POLL: Duration = Duration::from_millis(250);
const ADMISSION_GRACE: Duration = Duration::from_millis(500);
const DEFAULT_WAIT: u64 = 30_000;
const MAX_WAIT: u64 = 120_000;

pub(crate) struct Tasks {
    workspace: Arc<Workspace>,
    peers: Arc<Peers>,
    changed: broadcast::Sender<()>,
    scan: Notify,
    shutdown: CancellationToken,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    /// Serializes a cancellation's entry-only decision against delivery of its
    /// descendants.
    delivery: AsyncMutex<()>,
    /// Tasks whose delegating call is still waiting to see them admitted. A
    /// refusal the call itself reports is not delivered again.
    awaited: Arc<Mutex<HashSet<TaskId>>>,
}

/// Keeps a task in [`Tasks::awaited`].
struct Awaited {
    awaited: Arc<Mutex<HashSet<TaskId>>>,
    id: TaskId,
}

impl Drop for Awaited {
    fn drop(&mut self) {
        self.awaited
            .lock()
            .expect("awaited tasks lock poisoned")
            .remove(&self.id);
    }
}

impl std::fmt::Debug for Tasks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Tasks").finish_non_exhaustive()
    }
}

handlers! {
    ROUTES for Tasks {
        ListTasks, |tasks, ListTasks { parent }| {
            Ok(TaskList { tasks: tasks.list(parent).await? })
        }
        DelegateTask, |tasks, ctx, DelegateTask { parent, input }| {
            Ok(TaskResult { task: tasks.delegate(&ctx.principal.key(), parent, None, input).await? })
        }
        TaskStatusRequest, |tasks, TaskStatusRequest { task, wait, timeout_ms }| {
            tasks.status(&task, None, wait, timeout_ms).await
        }
        CancelTask, |tasks, CancelTask { task }| {
            Ok(TaskResult { task: tasks.cancel(&task, None).await? })
        }
        DeleteTask, |tasks, DeleteTask { task }| {
            tasks.delete(&task, None).await?;
            Ok(Empty {})
        }
        ClearTasks, |tasks, ClearTasks { parent }| {
            Ok(Cleared { count: tasks.clear(parent).await? })
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
enum Job {
    Task(TaskId),
    FollowUp {
        environment: String,
        task: TaskId,
        run: RunId,
    },
    Stop {
        thread: ThreadId,
        run: RunId,
    },
}

impl Tasks {
    pub(crate) fn new(workspace: Arc<Workspace>, peers: Arc<Peers>) -> Arc<Self> {
        Arc::new(Self {
            workspace,
            peers,
            changed: broadcast::channel(64).0,
            scan: Notify::new(),
            shutdown: CancellationToken::new(),
            supervisor: Mutex::new(None),
            delivery: AsyncMutex::new(()),
            awaited: Arc::new(Mutex::default()),
        })
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changed.subscribe()
    }

    pub(crate) fn resume(self: &Arc<Self>) {
        let mut supervisor = self
            .supervisor
            .lock()
            .expect("task supervisor lock poisoned");
        if supervisor.is_none() && !self.shutdown.is_cancelled() {
            let tasks = Arc::clone(self);
            *supervisor = Some(tokio::spawn(async move { tasks.supervise().await }));
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let supervisor = self
            .supervisor
            .lock()
            .expect("task supervisor lock poisoned")
            .take();
        if let Some(supervisor) = supervisor {
            let _ = supervisor.await;
        }
    }

    pub(crate) async fn list(&self, parent: Option<ThreadId>) -> Result<Vec<Task>, Error> {
        let stored = self
            .workspace
            .db
            .read(move |conn| store::list(conn, parent))
            .await?;
        Ok(stored.into_iter().map(|task| self.task(task)).collect())
    }

    pub(crate) async fn delegate(
        &self,
        principal_key: &str,
        parent: ThreadId,
        turn: Option<RunId>,
        input: DelegateInput,
    ) -> Result<Task, Error> {
        let environment = input
            .environment
            .clone()
            .unwrap_or_else(|| self.peers.local.clone());
        if environment != self.peers.local && self.peers.get(&environment).is_none() {
            return Err(Rejection::UnknownEnvironment(environment).into());
        }
        decisions::validate_request(&input.text, input.request_id.as_deref())?;
        let title = decisions::title(input.title.as_deref(), &input.text)?;
        if let Some(request_id) = input.request_id.clone()
            && let Some(id) = self
                .workspace
                .db
                .read(move |conn| prior_request(conn, parent, &request_id))
                .await?
        {
            return self.admitted(&id, parent).await;
        }
        let attachments = self
            .workspace
            .prepare_delegated_attachments(
                principal_key,
                parent,
                input.attachments,
                &input.attachment_ids,
            )
            .await?;

        let local_env = self.peers.local.clone();
        let awaited_set = Arc::clone(&self.awaited);
        let (id, awaited, created) = self
            .workspace
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                let prior = if let Some(request_id) = input.request_id.as_deref() {
                    prior_request(&tx, parent, request_id)?
                } else {
                    None
                };
                if let Some(id) = prior {
                    return Ok((id, None, false));
                }
                let active = active_run_from(&tx, parent)?;
                let stopping = match active {
                    Some(run) => stop_exists(&tx, parent, run)?,
                    None => false,
                };
                let parent_run = decisions::open(Delegator { active, stopping }, turn)?;
                let id = TaskId(Uuid::new_v4().to_string());
                let payload = serde_json::to_string(&ReceiveTask {
                    task: id.clone(),
                    origin_environment: local_env,
                    parent,
                    thread: input.thread,
                    target: TaskTarget {
                        project: input.project,
                        worktree: input.worktree,
                        harness: input.harness,
                        model: input.model,
                        thinking: input.thinking,
                    },
                    title: input.title,
                    text: input.text,
                    attachments,
                })?;
                store::insert(
                    &tx,
                    New {
                        id: &id,
                        parent,
                        parent_run,
                        environment: &environment,
                        title: &title,
                        payload: &payload,
                        request_id: input.request_id.as_deref(),
                    },
                )?;
                tx.commit()?;
                let awaited = turn.map(|_| hold_admission(&awaited_set, &id));
                Ok((id, awaited, true))
            })
            .await?;
        if created {
            self.signal();
            self.scan.notify_one();
        }
        let task = self.admitted(&id, parent).await?;
        if awaited.is_some() {
            self.apply(&id, Command::Seen).await?;
        }
        drop(awaited);
        Ok(task)
    }

    fn held(&self, id: &TaskId) -> bool {
        self.awaited
            .lock()
            .expect("awaited tasks lock poisoned")
            .contains(id)
    }

    /// Remove a settled task from its parent's list.
    pub(crate) async fn delete(
        &self,
        task: &TaskId,
        parent: Option<ThreadId>,
    ) -> Result<(), Error> {
        let task = task.clone();
        self.workspace
            .db
            .write_tx(move |tx| {
                let stored = store::read(tx, &task, parent)?;
                store::save(tx, &task, &decisions::dismiss(&stored.state)?)
            })
            .await?;
        self.signal();
        Ok(())
    }

    /// Remove every removable task from a thread's list.
    pub(crate) async fn clear(&self, parent: ThreadId) -> Result<usize, Error> {
        let count = self
            .workspace
            .db
            .write_tx(move |tx| {
                let mut count = 0;
                for task in store::list(tx, Some(parent))? {
                    if let Ok(next) = decisions::dismiss(&task.state) {
                        store::save(tx, &task.id, &next)?;
                        count += 1;
                    }
                }
                Ok(count)
            })
            .await?;
        self.signal();
        Ok(count)
    }

    pub(crate) async fn status(
        &self,
        task: &TaskId,
        parent: Option<ThreadId>,
        wait: bool,
        timeout_ms: Option<u64>,
    ) -> Result<TaskStatusResult, Error> {
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_WAIT);
        if timeout_ms > MAX_WAIT {
            return Err(Error::invalid("task wait is limited to 120 seconds"));
        }
        if !wait {
            return Ok(TaskStatusResult {
                task: self.read(task, parent).await?,
                timed_out: false,
            });
        }
        self.wait_for(task, parent, Duration::from_millis(timeout_ms), |task| {
            is_settled(task.status)
        })
        .await
    }

    pub(crate) async fn cancel(
        &self,
        task: &TaskId,
        parent: Option<ThreadId>,
    ) -> Result<Task, Error> {
        let task_owned = task.clone();
        let current = self
            .workspace
            .db
            .read(move |conn| store::read(conn, &task_owned, parent))
            .await?;
        if is_settled(current.state.status) {
            return Ok(self.task(current));
        }
        self.apply(task, Command::Cancel).await?;
        self.scan.notify_one();
        Ok(self
            .wait_for(task, parent, ADMISSION_GRACE, |record| {
                is_settled(record.status)
            })
            .await?
            .task)
    }

    async fn admitted(&self, id: &TaskId, parent: ThreadId) -> Result<Task, Error> {
        Ok(self
            .wait_for(id, Some(parent), ADMISSION_GRACE, |task| {
                task.thread.is_some() || is_settled(task.status)
            })
            .await?
            .task)
    }

    async fn wait_for(
        &self,
        id: &TaskId,
        parent: Option<ThreadId>,
        timeout: Duration,
        done: impl Fn(&Task) -> bool,
    ) -> Result<TaskStatusResult, Error> {
        let mut changes = self.subscribe();
        let deadline = Instant::now() + timeout;
        loop {
            let task = self.read(id, parent).await?;
            let complete = done(&task);
            if complete || Instant::now() >= deadline {
                return Ok(TaskStatusResult {
                    task,
                    timed_out: !complete,
                });
            }
            let _ = time::timeout_at(deadline, changes.recv()).await;
        }
    }

    async fn read(&self, id: &TaskId, parent: Option<ThreadId>) -> Result<Task, Error> {
        let id = id.clone();
        let stored = self
            .workspace
            .db
            .read(move |conn| store::read(conn, &id, parent))
            .await?;
        Ok(self.task(stored))
    }

    fn task(&self, stored: Stored) -> Task {
        let receipt = stored.state.phase.receipt();
        Task {
            id: stored.id.0,
            parent: stored.parent,
            reachable: self.peers.reachable(&stored.environment),
            environment: stored.environment,
            thread: receipt.map(|receipt| receipt.thread),
            run: receipt.map(|receipt| receipt.run),
            status: stored.state.status,
            title: stored.title,
            result: stored.state.result,
            error: stored.state.error,
        }
    }

    /// Tasks count in their parents' summaries, so the thread list changes
    /// too.
    fn signal(&self) {
        let _ = self.changed.send(());
        self.workspace.directory_changed();
    }
}

/// Hold back the task's delivery while its delegating call waits.
fn hold_admission(awaited: &Arc<Mutex<HashSet<TaskId>>>, id: &TaskId) -> Awaited {
    awaited
        .lock()
        .expect("awaited tasks lock poisoned")
        .insert(id.clone());
    Awaited {
        awaited: Arc::clone(awaited),
        id: id.clone(),
    }
}
