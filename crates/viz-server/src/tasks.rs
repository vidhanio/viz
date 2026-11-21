//! Task ownership, delivery, and cancellation survive restarts and peer
//! partitions.

pub(crate) mod model;

use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use toasty::Executor;
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, broadcast},
    task::JoinHandle,
    time::{self, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use viz_client::protocol::{TaskList, TaskResult, TaskStatusResult};

use self::model::{StoppedTaskRun, TaskRow};
use crate::{
    environments::Peers,
    wire::{
        Attachment, INVALID_PARAMS, NOT_FOUND, RpcError, Run, RunId, RunStatus, Task, TaskStatus,
        ThreadId, decode, encode,
    },
    workspace::{WakeMode, Workspace, active_run_from, run_from, sql},
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

// Offered may already be admitted at the recipient, even if no receipt reached
// us. Cancelling after an ambiguous offer must retract before it can stop any
// admitted run.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Pending,
    Offered,
    Retracting,
    Accepted,
    Cancelling,
    Settled,
}

impl Phase {
    fn parse(value: &str) -> Result<Self, RpcError> {
        match value {
            "pending" => Ok(Self::Pending),
            "offered" => Ok(Self::Offered),
            "retracting" => Ok(Self::Retracting),
            "accepted" => Ok(Self::Accepted),
            "cancelling" => Ok(Self::Cancelling),
            "settled" => Ok(Self::Settled),
            _ => Err(RpcError::internal(format!(
                "invalid persisted task phase {value}"
            ))),
        }
    }
}

macro_rules! cancel_task_sql {
    ($tail:literal) => {
        concat!(
            "UPDATE agent_tasks SET phase = CASE phase ",
            "WHEN 'pending' THEN 'settled' WHEN 'offered' THEN 'retracting' ",
            "WHEN 'accepted' THEN 'cancelling' ELSE phase END, ",
            "status = CASE phase WHEN 'pending' THEN 'cancelled' ELSE status END, ",
            "payload = CASE phase WHEN 'pending' THEN NULL ELSE payload END ",
            $tail,
        )
    };
}

// A delegated run includes only the descendants created in that run and the
// runs woken by those descendants, never manually queued follow-ups.
macro_rules! family_sql {
    ($query:expr) => {
        concat!(
            "WITH RECURSIVE lineage(id) AS (SELECT ?2 UNION SELECT a.wake_run ",
            "FROM agent_tasks a JOIN lineage p ON a.parent_run = p.id ",
            "WHERE a.parent = ?1 AND a.wake_run IS NOT NULL) ",
            $query,
        )
    };
}

pub(crate) struct Tasks {
    workspace: Arc<Workspace>,
    peers: Arc<Peers>,
    changed: broadcast::Sender<()>,
    scan: Notify,
    shutdown: CancellationToken,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    // Serialize a cancellation's entry-only decision against delivery of its
    // descendants.
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
    Stop { thread: u64, run: u64 },
}

struct Family {
    outcome: Option<Run>,
    active: Vec<u64>,
    pending_delivery: bool,
    cancelled_run: bool,
}

impl Family {
    const fn pending(&self) -> bool {
        self.pending_delivery || !self.active.is_empty()
    }
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
            _ => Err(RpcError::new(
                -32601,
                format!("unknown task method {method}"),
            )),
        }
    }

    pub(crate) async fn list(&self, parent: Option<u64>) -> Result<Vec<Task>, RpcError> {
        let mut db = self.workspace.db.lock().await;
        let rows = sql::rows(&mut *db, LIST_TASKS_SQL, [parent.into()]).await?;
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
            if let Some(id) = prior_request(&mut *db, parent, request_id).await? {
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
                prior_request(&mut *db, parent, request_id).await?
            } else {
                None
            };
            if let Some(id) = prior {
                id
            } else {
                let parent_run = active_run_from(&mut *db, parent).await?;
                if turn.is_some() && turn != parent_run {
                    return Err(RpcError::invalid("the delegating run is no longer active"));
                }
                if let Some(run) = parent_run
                    && stop_exists(&mut *db, parent, run).await?
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
                    harness: input.harness.as_deref(),
                    model: input.model.as_deref(),
                    thinking: input.thinking.as_deref(),
                    text: &input.text,
                    attachments: &input.attachments,
                })
                .map_err(RpcError::internal)?;
                let title = task_title(&input.text);
                let mut create = TaskRow::create()
                    .id(&id)
                    .parent(parent)
                    .environment(environment)
                    .title(title)
                    .payload(payload);
                if let Some(run) = parent_run {
                    create = create.parent_run(run);
                }
                if let Some(request_id) = input.request_id.as_deref() {
                    create = create.request_id(request_id);
                }
                create.exec(&mut *db).await?;
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
            let current = self.read_from(&mut *db, task, parent).await?;
            if !terminal(current.status) {
                sql::exec(&mut *db, cancel_task_sql!("WHERE id = ?1"), [task.into()]).await?;
            }
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
        db: &mut dyn Executor,
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

    async fn supervise(self: Arc<Self>) {
        let mut jobs: HashMap<Job, JoinHandle<()>> = HashMap::new();
        let mut poll = time::interval(POLL);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                _ = poll.tick() => {},
                () = self.scan.notified() => {},
            }
            jobs.retain(|_, worker| !worker.is_finished());
            match self.pending_jobs().await {
                Ok(pending) => {
                    for job in pending {
                        jobs.entry(job)
                            .or_insert_with_key(|job| self.start_job(job));
                    }
                }
                Err(error) => tracing::error!(%error, "scanning durable tasks"),
            }
        }
        for (_, worker) in jobs {
            let _ = worker.await;
        }
    }

    async fn pending_jobs(&self) -> Result<Vec<Job>, RpcError> {
        let mut db = self.workspace.db.lock().await;
        let mut jobs = sql::rows(
            &mut *db,
            "SELECT id FROM agent_tasks WHERE delivery != 'delivered'",
            [],
        )
        .await?
        .map(|mut row| Ok(Job::Task(row.str(0)?)))
        .collect::<Result<Vec<_>, RpcError>>()?;
        for row in sql::rows(
            &mut *db,
            "SELECT thread, run FROM stopped_task_runs WHERE handled = 0",
            [],
        )
        .await?
        {
            jobs.push(Job::Stop {
                thread: row.u64(0)?,
                run: row.u64(1)?,
            });
        }
        drop(db);
        Ok(jobs)
    }

    fn start_job(self: &Arc<Self>, job: &Job) -> JoinHandle<()> {
        let tasks = Arc::clone(self);
        match job {
            Job::Task(id) => {
                let id = id.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        () = tasks.drive_task(&id) => {},
                    }
                })
            }
            Job::Stop { thread, run } => {
                let (thread, run) = (*thread, *run);
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        () = tasks.drive_stop(thread, run) => {},
                    }
                })
            }
        }
    }

    async fn drive_task(&self, id: &str) {
        loop {
            match self.advance(id).await {
                Ok(Advance::Done) => return,
                Ok(Advance::Immediate) => {}
                Ok(Advance::LocalPoll) => time::sleep(LOCAL_POLL).await,
                Ok(Advance::PeerPoll) => time::sleep(POLL).await,
                Err(error) => {
                    tracing::error!(task = id, %error, "advancing durable task");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    async fn advance(&self, id: &str) -> Result<Advance, RpcError> {
        let Some(mut row) = sql::row(&mut *self.workspace.db.lock().await,
            "SELECT environment, thread, run, status, phase FROM agent_tasks WHERE id = ?1 AND delivery != 'delivered'",
            [id.into()],
        ).await? else { return Ok(Advance::Done) };
        let phase = Phase::parse(&row.str(4)?)?;
        if phase == Phase::Settled {
            self.deliver(id).await?;
            return Ok(Advance::Done);
        }
        let target = row.str(0)?;
        let cancel = phase == Phase::Cancelling;
        let local = target == self.peers.local;
        let was_reachable = self.peers.reachable(&target);
        let poll = if local {
            Advance::LocalPoll
        } else {
            Advance::PeerPoll
        };
        if matches!(phase, Phase::Pending | Phase::Offered | Phase::Retracting) {
            return self.advance_admission(id, &target, phase, poll).await;
        }
        let (Some(thread), Some(run)) = (row.opt_u64(1)?, row.opt_u64(2)?) else {
            return Err(RpcError::internal("accepted task has no run receipt"));
        };
        let status = parse_status(&row.str(3)?)?;
        let request = ProgressRequest {
            thread,
            run,
            cancel,
        };
        let reply = if local {
            self.progress(request).await.map(|progress| progress.run)
        } else {
            self.peer::<ProgressResult>(
                &target,
                "task.progress",
                json!({"thread": thread, "run": run, "cancel": cancel}),
            )
            .await
            .map(|progress| progress.run)
        };
        if was_reachable != self.peers.reachable(&target) {
            self.signal();
        }
        match reply {
            Ok(Some(progress)) if progress.thread.0 == thread && progress.id.0 == run => {
                self.update_from_run(id, status, &progress, poll).await
            }
            Ok(Some(_)) => {
                self.peers.mark(&target, false);
                self.signal();
                Ok(poll)
            }
            Ok(None) => {
                if cancel {
                    self.finish(id, "cancelled", None, None).await?;
                } else {
                    self.finish(
                        id,
                        "failed",
                        None,
                        Some("the assigned run no longer exists"),
                    )
                    .await?;
                }
                Ok(Advance::Immediate)
            }
            Err(error) if local => Err(error),
            Err(_) => Ok(poll),
        }
    }

    async fn advance_admission(
        &self,
        id: &str,
        target: &str,
        phase: Phase,
        poll: Advance,
    ) -> Result<Advance, RpcError> {
        let was_reachable = self.peers.reachable(target);
        let retracting = phase == Phase::Retracting;
        let result = if retracting {
            self.destination::<Option<Receipt>>(
                target,
                "task.retract",
                json!({"originEnvironment": self.peers.local, "task": id}),
            )
            .await
        } else {
            let payload = {
                let mut db = self.workspace.db.lock().await;
                if phase == Phase::Pending {
                    let changed = sql::exec(&mut *db, "UPDATE agent_tasks SET phase = 'offered' WHERE id = ?1 AND phase = 'pending'", [id.into()]).await?;
                    if changed == 0 {
                        return Ok(Advance::Immediate);
                    }
                }
                let payload = sql::row(
                    &mut *db,
                    "SELECT payload FROM agent_tasks WHERE id = ?1 AND phase = 'offered'",
                    [id.into()],
                )
                .await?;
                drop(db);
                let Some(mut row) = payload else {
                    return Ok(Advance::Immediate);
                };
                row.str(0)?
            };
            let params = serde_json::from_str(&payload).map_err(RpcError::internal)?;
            self.destination::<Receipt>(target, "task.receive", params)
                .await
                .map(Some)
        };
        if was_reachable != self.peers.reachable(target) {
            self.signal();
        }
        match result {
            Ok(Some(receipt)) => {
                let mut db = self.workspace.db.lock().await;
                sql::exec(&mut *db, "UPDATE agent_tasks SET thread = ?2, run = ?3, payload = NULL, \
                    phase = CASE phase WHEN 'retracting' THEN 'cancelling' ELSE 'accepted' END WHERE id = ?1",
                    [id.into(), receipt.thread.get().into(), receipt.run.get().into()]).await?;
                drop(db);
                self.signal();
                Ok(Advance::Immediate)
            }
            Ok(None) if retracting => {
                self.finish(id, "cancelled", None, None).await?;
                Ok(Advance::Immediate)
            }
            Err(error) if !retracting && matches!(error.code, INVALID_PARAMS | NOT_FOUND) => {
                let mut db = self.workspace.db.lock().await;
                sql::exec(&mut *db,
                    "UPDATE agent_tasks SET status = CASE phase WHEN 'retracting' THEN 'cancelled' ELSE 'failed' END, \
                     error = CASE phase WHEN 'retracting' THEN NULL ELSE ?2 END, payload = NULL, phase = 'settled' WHERE id = ?1",
                    [id.into(), error.message.into()],
                ).await?;
                drop(db);
                self.signal();
                Ok(Advance::Immediate)
            }
            _ => Ok(poll),
        }
    }

    async fn update_from_run(
        &self,
        id: &str,
        previous: TaskStatus,
        run: &Run,
        poll: Advance,
    ) -> Result<Advance, RpcError> {
        match run.status {
            RunStatus::Queued | RunStatus::Running => {
                if previous == TaskStatus::Queued && run.status == RunStatus::Running {
                    sql::exec(
                        &mut *self.workspace.db.lock().await,
                        "UPDATE agent_tasks SET status = 'running' WHERE id = ?1",
                        [id.into()],
                    )
                    .await?;
                    self.signal();
                }
                Ok(poll)
            }
            RunStatus::Completed => {
                self.finish(id, "completed", run.answer.as_deref(), None)
                    .await?;
                Ok(Advance::Immediate)
            }
            RunStatus::Failed | RunStatus::Interrupted => {
                let error = if run.status == RunStatus::Interrupted {
                    "run interrupted"
                } else {
                    "run failed"
                };
                self.finish(id, "failed", None, run.error.as_deref().or(Some(error)))
                    .await?;
                Ok(Advance::Immediate)
            }
            RunStatus::Cancelled => {
                self.finish(id, "cancelled", None, None).await?;
                Ok(Advance::Immediate)
            }
        }
    }

    async fn peer<T: DeserializeOwned>(
        &self,
        target: &str,
        method: &str,
        params: Value,
    ) -> Result<T, RpcError> {
        let value = self.peers.call(target, method, params).await?;
        serde_json::from_value(value).map_err(|_| {
            self.peers.mark(target, false);
            RpcError::unavailable(format!(
                "invalid {method} response from environment {target}"
            ))
        })
    }

    async fn destination<T: DeserializeOwned>(
        &self,
        target: &str,
        method: &str,
        params: Value,
    ) -> Result<T, RpcError> {
        if target == self.peers.local {
            serde_json::from_value(self.workspace.call(method, params).await?)
                .map_err(RpcError::internal)
        } else {
            self.peer(target, method, params).await
        }
    }

    async fn finish(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), RpcError> {
        sql::exec(&mut *self.workspace.db.lock().await,
            "UPDATE agent_tasks SET status = ?2, result = ?3, error = ?4, payload = NULL, phase = 'settled' WHERE id = ?1",
            [id.into(), status.into(), result.into(), error.into()],
        ).await?;
        self.signal();
        Ok(())
    }

    async fn deliver(&self, id: &str) -> Result<(), RpcError> {
        let _delivery = self.delivery.lock().await;
        let mut row = sql::row(&mut *self.workspace.db.lock().await,
            "SELECT parent, environment, thread, status, title, result, error, delivery FROM agent_tasks WHERE id = ?1 AND delivery != 'delivered'",
            [id.into()],
        ).await?.ok_or_else(|| RpcError::internal("task vanished before delivery"))?;
        let target = row.str(1)?;
        let location = match row.opt_u64(2)? {
            Some(child) => format!("{target} thread {child}"),
            None => target,
        };
        let status = row.str(3)?;
        let title = row.str(4)?;
        let details = row.opt_str(5)?.or(row.opt_str(6)?);
        let mut text = format!("Task {id} ({title}) on {location} {status}.");
        if let Some(details) = details {
            text.push_str("\n\n");
            text.push_str(&details);
        }
        let mode = match row.str(7)?.as_str() {
            "entry_only" => WakeMode::EntryOnly,
            "prompt" => WakeMode::Prompt,
            mode => {
                return Err(RpcError::internal(format!(
                    "invalid persisted task delivery {mode}"
                )));
            }
        };
        // The workspace commits the entry and its wake receipt together. If we
        // crash before recording delivered=1, the same durable wake run is
        // returned on retry.
        let wake = self.workspace.wake(row.u64(0)?, id, &text, mode).await?;
        sql::exec(
            &mut *self.workspace.db.lock().await,
            "UPDATE agent_tasks SET delivery = 'delivered', wake_run = ?2 WHERE id = ?1",
            [id.into(), wake.into()],
        )
        .await?;
        self.signal();
        Ok(())
    }

    async fn progress(&self, request: ProgressRequest) -> Result<ProgressResult, RpcError> {
        let ProgressRequest {
            thread,
            run,
            cancel,
        } = request;
        if thread == 0 || run == 0 {
            return Err(RpcError::invalid("invalid thread or run ID"));
        }
        let _delivery = self.delivery.lock().await;
        let mut db = self.workspace.db.lock().await;
        let family = snapshot(&mut *db, thread, run).await?;
        let mut stopped = stop_exists(&mut *db, thread, run).await?;
        if (cancel || family.cancelled_run) && family.pending() && !stopped {
            let mut tx = db.transaction().await?;
            StoppedTaskRun::create()
                .thread(thread)
                .run(run)
                .exec(&mut tx)
                .await?;
            suppress_descendant_wakes(&mut tx, thread, run).await?;
            tx.commit().await?;
            stopped = true;
            self.scan.notify_one();
            self.signal();
        }
        drop(db);
        let pending = family.pending();
        let run = if pending {
            family
                .outcome
                .filter(|outcome| run_open(outcome.status))
                .or(Some(Run {
                    id: RunId(run),
                    thread: ThreadId(thread),
                    status: RunStatus::Running,
                    answer: None,
                    error: None,
                }))
        } else {
            family.outcome.map(|mut outcome| {
                if stopped || family.cancelled_run {
                    outcome.status = RunStatus::Cancelled;
                    outcome.answer = None;
                    outcome.error = None;
                }
                outcome
            })
        };
        Ok(ProgressResult { run })
    }

    async fn drive_stop(&self, thread: u64, run: u64) {
        loop {
            match self.stop_family(thread, run).await {
                Ok(true) => return,
                Ok(false) => time::sleep(LOCAL_POLL).await,
                Err(error) => {
                    tracing::error!(thread, run, %error, "cancelling delegated run tree");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    async fn stop_family(&self, thread: u64, run: u64) -> Result<bool, RpcError> {
        let active = {
            let _delivery = self.delivery.lock().await;
            let mut db = self.workspace.db.lock().await;
            let family = snapshot(&mut *db, thread, run).await?;
            if !family.pending() {
                sql::exec(
                    &mut *db,
                    "UPDATE stopped_task_runs SET handled = 1 WHERE thread = ?1 AND run = ?2",
                    [thread.into(), run.into()],
                )
                .await?;
                return Ok(true);
            }
            suppress_descendant_wakes(&mut *db, thread, run).await?;
            drop(db);
            family.active
        };
        for active_run in active {
            if let Err(error) = self
                .workspace
                .call("thread.stop", json!({"thread": thread, "run": active_run}))
                .await
                && !matches!(error.code, INVALID_PARAMS | NOT_FOUND)
            {
                return Err(error);
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Copy)]
enum Advance {
    Done,
    Immediate,
    LocalPoll,
    PeerPoll,
}

async fn prior_request(
    db: &mut dyn Executor,
    parent: u64,
    request: &str,
) -> Result<Option<String>, RpcError> {
    sql::row(
        db,
        "SELECT id FROM agent_tasks WHERE parent = ?1 AND request_id = ?2",
        [parent.into(), request.into()],
    )
    .await?
    .map(|mut row| row.str(0))
    .transpose()
}

async fn stop_exists(db: &mut dyn Executor, thread: u64, run: u64) -> Result<bool, RpcError> {
    Ok(sql::row(
        db,
        "SELECT handled FROM stopped_task_runs WHERE thread = ?1 AND run = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .is_some())
}

async fn suppress_descendant_wakes(
    db: &mut dyn Executor,
    thread: u64,
    run: u64,
) -> Result<(), RpcError> {
    sql::exec(
        db,
        family_sql!(cancel_task_sql!(
            ", delivery = 'entry_only' WHERE parent = ?1 \
         AND parent_run IN (SELECT id FROM lineage) AND delivery = 'prompt'"
        )),
        [thread.into(), run.into()],
    )
    .await?;
    Ok(())
}

async fn snapshot(db: &mut dyn Executor, thread: u64, run: u64) -> Result<Family, RpcError> {
    let mut root = run_from(db, thread, run).await?;
    let rows = sql::rows(
        db,
        family_sql!(
            "SELECT a.delivery = 'delivered', r.id, r.status FROM agent_tasks a \
         LEFT JOIN runs r ON r.id = a.wake_run AND r.thread = a.parent \
         WHERE a.parent = ?1 AND a.parent_run IN (SELECT id FROM lineage) ORDER BY r.id DESC"
        ),
        [thread.into(), run.into()],
    )
    .await?;
    let mut active = Vec::new();
    let mut cancelled_run = root
        .as_ref()
        .is_some_and(|root| root.status == RunStatus::Cancelled);
    if root.as_ref().is_some_and(|root| run_open(root.status)) {
        active.push(run);
    }
    let mut latest = None;
    let mut pending_delivery = false;
    for mut row in rows {
        pending_delivery |= !row.flag(0)?;
        if let Some(wake) = row.opt_u64(1)? {
            let status = row.str(2)?;
            if matches!(status.as_str(), "queued" | "running") {
                active.push(wake);
            }
            cancelled_run |= status == "cancelled";
            latest = latest.or(Some(wake));
        }
    }
    if active.is_empty()
        && !pending_delivery
        && !cancelled_run
        && let (Some(outcome), Some(latest)) = (&mut root, latest)
        && let Some(settled) = run_from(db, thread, latest).await?
    {
        outcome.status = settled.status;
        outcome.answer = settled.answer;
        outcome.error = settled.error;
    }
    Ok(Family {
        outcome: root,
        active,
        pending_delivery,
        cancelled_run,
    })
}

const fn run_open(status: RunStatus) -> bool {
    matches!(status, RunStatus::Queued | RunStatus::Running)
}

const fn terminal(status: TaskStatus) -> bool {
    !matches!(status, TaskStatus::Queued | TaskStatus::Running)
}

fn parse_status(status: &str) -> Result<TaskStatus, RpcError> {
    match status {
        "queued" => Ok(TaskStatus::Queued),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        _ => Err(RpcError::internal(format!(
            "invalid persisted task status {status}"
        ))),
    }
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
