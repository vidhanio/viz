//! Durable delegation endpoints. The supervisor runs persisted work from the
//! store.

mod delivery;
mod drive;
mod store;

use std::{
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
use viz_protocol::{
    CancelTask, DelegateTask, ListTasks, TaskId, TaskList, TaskResult, TaskStatusRequest,
    TaskStatusResult, WorktreeTarget,
    peer::{ReceiveTask, TaskTarget},
};

use self::store::{cancel_task, parse_status, prior_request, stop_exists, task_phase};
use crate::{
    environments::Peers,
    error::Error,
    rpc::handlers,
    wire::{Attachment, RunId, Task, TaskStatus, ThreadId},
    workspace::{Workspace, active_run_from, sql, sql::Connection},
};

const POLL: Duration = Duration::from_secs(1);
const LOCAL_POLL: Duration = Duration::from_millis(250);
const ADMISSION_GRACE: Duration = Duration::from_millis(500);
const DEFAULT_WAIT: u64 = 30_000;
const MAX_WAIT: u64 = 120_000;
const LIST_TASKS_SQL: &str = concat!(
    "SELECT id, parent, environment, thread, run, status, title, result, error ",
    "FROM agent_tasks WHERE (?1 IS NULL OR parent = ?1) ORDER BY rowid DESC"
);
const READ_TASK_SQL: &str = concat!(
    "SELECT id, parent, environment, thread, run, status, title, result, error ",
    "FROM agent_tasks WHERE id = ?1 AND (?2 IS NULL OR parent = ?2)"
);

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
pub(crate) struct CancelRequest {
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
        CancelTask<'static>, CancelRequest, |tasks, CancelRequest { task }| {
            Ok(TaskResult { task: tasks.cancel(&task, None).await? })
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
        let mut db = self.workspace.db.lock().await;
        let rows = sql::rows(&mut db, LIST_TASKS_SQL, [parent.into()]).await?;
        drop(db);
        rows.map(|row| self.task(row)).collect()
    }

    pub(crate) async fn delegate(
        &self,
        parent: ThreadId,
        turn: Option<RunId>,
        mut input: Delegate,
    ) -> Result<Task, Error> {
        let environment = input.environment.as_deref().unwrap_or(&self.peers.local);
        if environment != self.peers.local && self.peers.get(environment).is_none() {
            return Err(Error::invalid(format!("unknown environment {environment}")));
        }
        if input.text.trim().is_empty() {
            return Err(Error::invalid(
                "task requires a parent, a valid target, and nonempty text",
            ));
        }
        if input
            .request_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(Error::invalid("task requestId must contain 1–256 bytes"));
        }
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

        let id = {
            let mut db = self.workspace.db.lock().await;
            let prior = if let Some(request_id) = input.request_id.as_deref() {
                prior_request(&mut db, parent, request_id).await?
            } else {
                None
            };
            if let Some(id) = prior {
                id
            } else {
                let parent_run = active_run_from(&mut db, parent).await?;
                if turn.is_some() && turn != parent_run {
                    return Err(Error::invalid("the delegating run is no longer active"));
                }
                if let Some(run) = parent_run
                    && stop_exists(&mut db, parent, run).await?
                {
                    return Err(Error::invalid("the delegating run is being cancelled"));
                }
                let id = TaskId(Uuid::new_v4().to_string());
                let title = task_title(&input.text);
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
                    text: input.text,
                    attachments: input.attachments,
                })?;
                sql::exec(
                    &mut db,
                    "INSERT INTO agent_tasks (id, parent, parent_run, environment, status, title, payload, request_id, phase, delivery)
                     VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, 'pending', 'prompt')",
                    [
                        (&id).into(), parent.into(), parent_run.into(), environment.into(),
                        title.into(), payload.into(), input.request_id.as_deref().into(),
                    ],
                )
                .await?;
                drop(db);
                self.signal();
                self.scan.notify_one();
                id
            }
        };
        self.admitted(&id, parent).await
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
            terminal(task.status)
        })
        .await
    }

    pub(crate) async fn cancel(
        &self,
        task: &TaskId,
        parent: Option<ThreadId>,
    ) -> Result<Task, Error> {
        let current = {
            let mut db = self.workspace.db.lock().await;
            let mut tx = db.transaction().await?;
            let current = self.read_from(&mut tx, task, parent).await?;
            if !terminal(current.status)
                && let Some(phase) = task_phase(&mut tx, task).await?
            {
                cancel_task(&mut tx, task, phase, None).await?;
            }
            tx.commit().await?;
            drop(db);
            current
        };
        if terminal(current.status) {
            return Ok(current);
        }
        self.signal();
        self.scan.notify_one();
        Ok(self
            .wait_for(task, parent, ADMISSION_GRACE, |record| {
                terminal(record.status)
            })
            .await?
            .task)
    }

    async fn admitted(&self, id: &TaskId, parent: ThreadId) -> Result<Task, Error> {
        Ok(self
            .wait_for(id, Some(parent), ADMISSION_GRACE, |task| {
                task.thread.is_some() || terminal(task.status)
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
        self.read_from(&mut *self.workspace.db.lock().await, id, parent)
            .await
    }

    async fn read_from(
        &self,
        db: &mut Connection,
        id: &TaskId,
        parent: Option<ThreadId>,
    ) -> Result<Task, Error> {
        let row = sql::row(db, READ_TASK_SQL, [id.into(), parent.into()])
            .await?
            .ok_or_else(|| Error::NoTask(id.clone()))?;
        self.task(row)
    }

    fn task(&self, mut row: sql::Row) -> Result<Task, Error> {
        let environment = row.str(2)?;
        Ok(Task {
            id: row.str(0)?,
            parent: row.id(1)?,
            reachable: self.peers.reachable(&environment),
            environment,
            thread: row.opt_id(3)?,
            run: row.opt_id(4)?,
            status: parse_status(&row.str(5)?)?,
            title: row.str(6)?,
            result: row.opt_str(7)?,
            error: row.opt_str(8)?,
        })
    }

    fn signal(&self) {
        let _ = self.changed.send(());
    }
}

const fn terminal(status: TaskStatus) -> bool {
    !matches!(status, TaskStatus::Queued | TaskStatus::Running)
}

fn task_title(text: &str) -> String {
    text.trim()
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect()
}
