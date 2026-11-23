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

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, broadcast},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use viz_core::tasks::{self as decisions, Command, Delegator, Rejection, is_settled};
use viz_protocol::{
    CancelTask, ClearTasks, Cleared, DelegateTask, DeleteTask, Empty, ListTasks, TaskId, TaskList,
    TaskResult, TaskStatusRequest, TaskStatusResult, WorktreeTarget,
    peer::{ReceiveTask, TaskTarget},
};

use self::store::{New, Stored, prior_request, stop_exists};
use crate::{
    environments::Peers,
    error::Error,
    rpc::handlers,
    wire::{Attachment, RunId, Task, ThreadId},
    workspace::{Workspace, active_run_from},
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
    awaited: Mutex<HashSet<TaskId>>,
}

/// Keeps a task in [`Tasks::awaited`].
struct Awaited<'a> {
    tasks: &'a Tasks,
    id: TaskId,
}

impl Drop for Awaited<'_> {
    fn drop(&mut self) {
        self.tasks
            .awaited
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Delegate {
    environment: Option<String>,
    project: Option<String>,
    /// Checkout of the project to run in: `{"mode": "new"}` isolates the
    /// child on a new branch. Omit to use `project`'s checkout, or the
    /// parent's checkout when `project` is omitted too.
    worktree: Option<WorktreeTarget>,
    harness: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    thread: Option<ThreadId>,
    #[schemars(length(min = 1))]
    text: String,
    #[serde(default)]
    attachments: Vec<Attachment>,
    #[serde(default)]
    attachment_ids: Vec<String>,
    #[schemars(length(min = 1, max = 256))]
    request_id: Option<String>,
    /// A short name for the task and its new thread. Defaults to the start of
    /// `text`.
    #[schemars(length(min = 1, max = 200))]
    title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListRequest {
    parent: Option<ThreadId>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StatusRequest {
    #[schemars(length(min = 1))]
    task: TaskId,
    #[serde(default)]
    wait: bool,
    #[schemars(range(max = 120_000))]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskRequest {
    #[schemars(length(min = 1))]
    task: TaskId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DelegateFromThread {
    parent: ThreadId,
    #[serde(flatten)]
    input: Delegate,
}

handlers! {
    ROUTES for Tasks {
        ListTasks, ListRequest, |tasks, ListRequest { parent }| {
            Ok(TaskList { tasks: tasks.list(parent).await? })
        }
        DelegateTask<'static>, DelegateFromThread, |tasks, DelegateFromThread { parent, input }| {
            Ok(TaskResult { task: tasks.delegate(parent, None, input).await? })
        }
        TaskStatusRequest<'static>, StatusRequest, |tasks, StatusRequest { task, wait, timeout_ms }| {
            tasks.status(&task, None, wait, timeout_ms).await
        }
        CancelTask<'static>, TaskRequest, |tasks, TaskRequest { task }| {
            Ok(TaskResult { task: tasks.cancel(&task, None).await? })
        }
        DeleteTask<'static>, TaskRequest, |tasks, TaskRequest { task }| {
            tasks.delete(&task, None).await?;
            Ok(Empty {})
        }
        ClearTasks, ClearTasks, |tasks, ClearTasks { parent }| {
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
            awaited: Mutex::default(),
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
        let stored = store::list(&mut *self.workspace.db.lock().await, parent).await?;
        Ok(stored.into_iter().map(|task| self.task(task)).collect())
    }

    pub(crate) async fn delegate(
        &self,
        parent: ThreadId,
        turn: Option<RunId>,
        mut input: Delegate,
    ) -> Result<Task, Error> {
        let environment = input.environment.as_deref().unwrap_or(&self.peers.local);
        if environment != self.peers.local && self.peers.get(environment).is_none() {
            return Err(Rejection::UnknownEnvironment(environment.to_owned()).into());
        }
        decisions::validate_request(&input.text, input.request_id.as_deref())?;
        let title = decisions::title(input.title.as_deref(), &input.text)?;
        if let Some(request_id) = input.request_id.as_deref() {
            let mut db = self.workspace.db.lock().await;
            if let Some(id) = prior_request(&mut db, parent, request_id).await? {
                drop(db);
                return self.admitted(&id, parent).await;
            }
        }
        if !input.attachment_ids.is_empty() {
            input.attachments.extend(
                self.workspace
                    .attachments(parent, &input.attachment_ids)
                    .await?,
            );
        }

        let (id, awaited) = {
            let mut db = self.workspace.db.lock().await;
            let prior = if let Some(request_id) = input.request_id.as_deref() {
                prior_request(&mut db, parent, request_id).await?
            } else {
                None
            };
            if let Some(id) = prior {
                (id, None)
            } else {
                let active = active_run_from(&mut db, parent).await?;
                let stopping = match active {
                    Some(run) => stop_exists(&mut db, parent, run).await?,
                    None => false,
                };
                let parent_run = decisions::open(Delegator { active, stopping }, turn)?;
                let id = TaskId(Uuid::new_v4().to_string());
                let payload = serde_json::to_string(&ReceiveTask {
                    task: id.clone(),
                    origin_environment: self.peers.local.clone(),
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
                    attachments: input.attachments,
                })?;
                let awaited = turn.map(|_| self.await_admission(&id));
                store::insert(
                    &mut db,
                    New {
                        id: &id,
                        parent,
                        parent_run,
                        environment,
                        title: &title,
                        payload: &payload,
                        request_id: input.request_id.as_deref(),
                    },
                )
                .await?;
                drop(db);
                self.signal();
                self.scan.notify_one();
                (id, awaited)
            }
        };
        let task = self.admitted(&id, parent).await?;
        if awaited.is_some() {
            self.apply(&id, Command::Seen).await?;
        }
        drop(awaited);
        Ok(task)
    }

    /// Hold back the task's delivery while its delegating call waits.
    fn await_admission(&self, id: &TaskId) -> Awaited<'_> {
        self.awaited
            .lock()
            .expect("awaited tasks lock poisoned")
            .insert(id.clone());
        Awaited {
            tasks: self,
            id: id.clone(),
        }
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
        let mut db = self.workspace.db.lock().await;
        let mut tx = db.transaction().await?;
        let stored = store::read(&mut tx, task, parent).await?;
        store::save(&mut tx, task, &decisions::dismiss(&stored.state)?).await?;
        tx.commit().await?;
        drop(db);
        self.signal();
        Ok(())
    }

    /// Remove every removable task from a thread's list.
    pub(crate) async fn clear(&self, parent: ThreadId) -> Result<usize, Error> {
        let mut db = self.workspace.db.lock().await;
        let mut tx = db.transaction().await?;
        let mut count = 0;
        for task in store::list(&mut tx, Some(parent)).await? {
            if let Ok(next) = decisions::dismiss(&task.state) {
                store::save(&mut tx, &task.id, &next).await?;
                count += 1;
            }
        }
        tx.commit().await?;
        drop(db);
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
        let current = store::read(&mut *self.workspace.db.lock().await, task, parent).await?;
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
        let stored = store::read(&mut *self.workspace.db.lock().await, id, parent).await?;
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

    fn signal(&self) {
        let _ = self.changed.send(());
    }
}
