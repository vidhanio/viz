//! Durable delegation endpoints. The supervisor runs persisted work from the
//! store.

mod delivery;
mod drive;
mod store;

use std::{
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, broadcast},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use viz_protocol::{TaskList, TaskResult, TaskStatusResult, WorktreeTarget};

use self::store::{cancel_task, parse_status, prior_request, stop_exists, task_phase};
use crate::{
    environments::Peers,
    wire::{Attachment, RpcError, Run, RunId, Task, TaskStatus, ThreadId, decode, encode},
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
    #[schemars(range(min = 1))]
    thread: Option<u64>,
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
struct ListRequest {
    #[schemars(range(min = 1))]
    parent: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatusRequest {
    #[schemars(length(min = 1))]
    task: String,
    #[serde(default)]
    wait: bool,
    #[schemars(range(max = 120_000))]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CancelRequest {
    #[schemars(length(min = 1))]
    task: String,
}

/// A settled continuation, identified by its run in the child environment.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FollowUp {
    source: String,
    task: String,
    thread: NonZeroU64,
    run: NonZeroU64,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProgressRequest {
    #[schemars(range(min = 1))]
    thread: u64,
    #[schemars(range(min = 1))]
    run: u64,
    #[serde(default)]
    cancel: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProgressResult {
    #[serde(deserialize_with = "required_option")]
    run: Option<Run>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdmissionRequest<'a> {
    task: &'a str,
    origin_environment: &'a str,
    parent: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<&'a WorktreeTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<&'a str>,
    text: &'a str,
    #[serde(skip_serializing_if = "<[Attachment]>::is_empty")]
    attachments: &'a [Attachment],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    thread: NonZeroU64,
    run: NonZeroU64,
}

#[derive(Debug, Eq, Hash, PartialEq)]
enum Job {
    Task(String),
    FollowUp {
        environment: String,
        task: String,
        run: u64,
    },
    Stop {
        thread: u64,
        run: u64,
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

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "tasks.list" => {
                let ListRequest { parent } = decode(params)?;
                let tasks = self.list(parent).await?;
                encode(TaskList { tasks })
            }
            "task.delegate" => {
                let Value::Object(mut params) = params else {
                    return Err(RpcError::invalid("expected task parameters"));
                };
                let parent = params
                    .remove("parent")
                    .and_then(|parent| parent.as_u64())
                    .ok_or_else(|| RpcError::invalid("task requires a parent thread"))?;
                let task = self
                    .delegate(parent, None, decode(Value::Object(params))?)
                    .await?;
                encode(TaskResult { task })
            }
            "task.status" => {
                let StatusRequest {
                    task,
                    wait,
                    timeout_ms,
                } = decode(params)?;
                encode(self.status(&task, None, wait, timeout_ms).await?)
            }
            "task.cancel" => {
                let CancelRequest { task } = decode(params)?;
                let task = self.cancel(&task, None).await?;
                encode(TaskResult { task })
            }
            "task.progress" => encode(self.progress(decode(params)?).await?),
            "task.follow_up" => encode(self.follow_up(decode(params)?).await?),
            _ => Err(RpcError::new(
                -32601,
                format!("unknown task method {method}"),
            )),
        }
    }

    pub(crate) async fn list(&self, parent: Option<u64>) -> Result<Vec<Task>, RpcError> {
        let mut db = self.workspace.db.lock().await;
        let rows = sql::rows(&mut db, LIST_TASKS_SQL, [parent.into()]).await?;
        drop(db);
        rows.map(|row| self.task(row)).collect()
    }

    pub(crate) async fn delegate(
        &self,
        parent: u64,
        turn: Option<u64>,
        mut input: Delegate,
    ) -> Result<Task, RpcError> {
        let environment = input.environment.as_deref().unwrap_or(&self.peers.local);
        if environment != self.peers.local && self.peers.get(environment).is_none() {
            return Err(RpcError::invalid(format!(
                "unknown environment {environment}"
            )));
        }
        if parent == 0 || input.thread == Some(0) || input.text.trim().is_empty() {
            return Err(RpcError::invalid(
                "task requires a parent, a valid target, and nonempty text",
            ));
        }
        if input
            .request_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256)
        {
            return Err(RpcError::invalid("task requestId must contain 1–256 bytes"));
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
                    return Err(RpcError::invalid("the delegating run is no longer active"));
                }
                if let Some(run) = parent_run
                    && stop_exists(&mut db, parent, run).await?
                {
                    return Err(RpcError::invalid("the delegating run is being cancelled"));
                }
                let id = Uuid::new_v4().to_string();
                let payload = serde_json::to_string(&AdmissionRequest {
                    task: &id,
                    origin_environment: &self.peers.local,
                    parent,
                    thread: input.thread,
                    project: input.project.as_deref(),
                    worktree: input.worktree.as_ref(),
                    harness: input.harness.as_deref(),
                    model: input.model.as_deref(),
                    thinking: input.thinking.as_deref(),
                    text: &input.text,
                    attachments: &input.attachments,
                })
                .map_err(RpcError::internal)?;
                let title = task_title(&input.text);
                sql::exec(
                    &mut db,
                    "INSERT INTO agent_tasks (id, parent, parent_run, environment, status, title, payload, request_id, phase, delivery)
                     VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, 'pending', 'prompt')",
                    [
                        id.as_str().into(), parent.into(), parent_run.into(), environment.into(),
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
        task: &str,
        parent: Option<u64>,
        wait: bool,
        timeout_ms: Option<u64>,
    ) -> Result<TaskStatusResult, RpcError> {
        let timeout_ms = timeout_ms.unwrap_or(DEFAULT_WAIT);
        if timeout_ms > MAX_WAIT {
            return Err(RpcError::invalid("task wait is limited to 120 seconds"));
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

    pub(crate) async fn cancel(&self, task: &str, parent: Option<u64>) -> Result<Task, RpcError> {
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

    async fn admitted(&self, id: &str, parent: u64) -> Result<Task, RpcError> {
        Ok(self
            .wait_for(id, Some(parent), ADMISSION_GRACE, |task| {
                task.thread.is_some() || terminal(task.status)
            })
            .await?
            .task)
    }

    async fn wait_for(
        &self,
        id: &str,
        parent: Option<u64>,
        timeout: Duration,
        done: impl Fn(&Task) -> bool,
    ) -> Result<TaskStatusResult, RpcError> {
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

    async fn read(&self, id: &str, parent: Option<u64>) -> Result<Task, RpcError> {
        self.read_from(&mut *self.workspace.db.lock().await, id, parent)
            .await
    }

    async fn read_from(
        &self,
        db: &mut Connection,
        id: &str,
        parent: Option<u64>,
    ) -> Result<Task, RpcError> {
        let row = sql::row(db, READ_TASK_SQL, [id.into(), parent.into()])
            .await?
            .ok_or_else(|| RpcError::not_found(format!("no task {id}")))?;
        self.task(row)
    }

    fn task(&self, mut row: sql::Row) -> Result<Task, RpcError> {
        let environment = row.str(2)?;
        Ok(Task {
            id: row.str(0)?,
            parent: ThreadId(row.u64(1)?),
            reachable: self.peers.reachable(&environment),
            environment,
            thread: row.opt_u64(3)?.map(ThreadId),
            run: row.opt_u64(4)?.map(RunId),
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

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
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
