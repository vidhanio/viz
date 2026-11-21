//! A local environment's ACP threads, durable run queue, project metadata, and
//! transcript.

mod attachments;
mod files;
mod rpc;
pub(crate) mod runs;
mod runtime;
pub(crate) mod sql;
mod store;
mod titles;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as SyncMutex},
    time::Duration,
};

use anyhow::Context as _;
use toasty::{Connection, Db};
use toasty_driver_sqlite::Sqlite;
use tokio::{
    sync::{Mutex, broadcast},
    time,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) use self::runs::{active_run_from, run_from};
use self::{
    files::Files,
    runs::NewRun,
    runtime::Driver,
    sql::{exec, row, rows},
};
use crate::{
    acp::Mcp,
    config::{Config, HarnessConfig},
    tasks::model::{StoppedTaskRun, TaskRow},
    wire::{
        Attachment, Entry, EntryPage, LiveTurn, NewEntry, Origin, RpcError, ThreadState, WaitResult,
    },
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    Thread(u64),
    Settled(u64),
    Directory,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum WatchChange {
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeMode {
    Prompt,
    EntryOnly,
}

#[derive(Default)]
struct Transient {
    threads: HashMap<u64, Arc<Driver>>,
    live: HashMap<u64, LiveTurn>,
    watches: HashMap<u64, usize>,
}

pub(crate) struct Workspace {
    /// Kept for the entire process lifetime: sole writer and the physical
    /// connection shared with tasks.
    pub(crate) db: Mutex<Connection>,
    pub(crate) environment: String,
    cwd: PathBuf,
    dir: PathBuf,
    default_harness: Option<String>,
    harnesses: BTreeMap<String, HarnessConfig>,
    changes: broadcast::Sender<Change>,
    transient: SyncMutex<Transient>,
    files: Files,
    mcp: Mcp,
    closing: CancellationToken,
    jobs: TaskTracker,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("environment", &self.environment)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    pub(crate) async fn open(config: &Config, mcp: Mcp) -> anyhow::Result<Arc<Self>> {
        let dir = config.state_dir.join(&config.environment);
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .await
            .with_context(|| format!("creating workspace directory {}", dir.display()))?;
        let pool = Db::builder()
            .models(toasty::models!(TaskRow, StoppedTaskRun))
            .max_pool_size(1)
            .build(Sqlite::open(dir.join("workspace.sqlite3")))
            .await?;
        let mut db = pool.connection().await?;
        // SQLite holds this lock after the initial write; two servers must
        // never drive the same queue.
        row(&mut db, "PRAGMA locking_mode = EXCLUSIVE", []).await?;
        row(&mut db, "PRAGMA journal_mode = WAL", []).await?;
        exec(&mut db, "PRAGMA foreign_keys = ON", []).await?;
        // The versioned workspace owns all DDL. Toasty's model registry
        // supplies typed queries and inserts, not a create-on-restart schema.
        let mut tx = db.transaction().await?;
        for ddl in include_str!("workspace/schema.sql").trim().split("\n\n") {
            exec(&mut tx, ddl, []).await?;
        }
        runs::recover(&mut tx).await?;
        tx.commit().await?;
        let (changes, _) = broadcast::channel(1024);
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            environment: config.environment.clone(),
            cwd: config.cwd.clone(),
            dir,
            default_harness: config.default_harness.clone(),
            harnesses: config.harnesses.clone(),
            changes,
            transient: SyncMutex::default(),
            files: Files::default(),
            mcp,
            closing: CancellationToken::new(),
            jobs: TaskTracker::new(),
        }))
    }

    pub(crate) async fn resume(self: &Arc<Self>) -> Result<(), RpcError> {
        for row in rows(
            &mut *self.db.lock().await,
            "SELECT DISTINCT thread FROM runs WHERE status = 'queued'",
            [],
        )
        .await?
        {
            self.kick(row.u64(0)?, false);
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        {
            let _runtime = self
                .transient
                .lock()
                .expect("workspace runtime mutex poisoned");
            self.closing.cancel();
            self.jobs.close();
        }
        self.jobs.wait().await;
        // Drop sessions outside the runtime mutex.
        let drivers = std::mem::take(
            &mut self
                .transient
                .lock()
                .expect("workspace runtime mutex poisoned")
                .threads,
        );
        drop(drivers);
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    fn changed(&self, thread: Option<u64>) {
        let _ = self
            .changes
            .send(thread.map_or(Change::Directory, Change::Thread));
    }

    fn relist(&self, thread: u64) {
        self.changed(Some(thread));
        self.changed(None);
    }

    fn settled(&self, thread: u64) {
        let _ = self.changes.send(Change::Settled(thread));
        self.changed(None);
    }

    pub(crate) async fn watched(&self, thread: u64, mode: WatchChange) -> Result<(), RpcError> {
        match mode {
            WatchChange::Open => {
                let mut db = self.db.lock().await;
                store::require(&mut *db, thread).await?;
                let updated = exec(
                    &mut *db,
                    "UPDATE threads SET unread = 0 WHERE id = ?1 AND unread = 1",
                    [thread.into()],
                )
                .await?;
                *self
                    .transient
                    .lock()
                    .expect("workspace runtime mutex poisoned")
                    .watches
                    .entry(thread)
                    .or_default() += 1;
                drop(db);
                if updated != 0 {
                    self.relist(thread);
                }
            }
            WatchChange::Close => {
                let mut state = self
                    .transient
                    .lock()
                    .expect("workspace runtime mutex poisoned");
                if let Some(watchers) = state.watches.get_mut(&thread) {
                    *watchers -= 1;
                    if *watchers == 0 {
                        state.watches.remove(&thread);
                    }
                }
            }
        }
        Ok(())
    }

    fn unread(&self, thread: u64) -> bool {
        !self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned")
            .watches
            .contains_key(&thread)
    }

    pub(crate) async fn state(&self, thread: u64) -> Result<ThreadState, RpcError> {
        let mut db = self.db.lock().await;
        let mut state = store::state(&mut *db, thread).await?;
        state.live = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned")
            .live
            .get(&thread)
            .cloned();
        drop(db);
        Ok(state)
    }

    pub(crate) async fn entries(
        &self,
        thread: u64,
        before: Option<u64>,
        limit: usize,
    ) -> Result<EntryPage, RpcError> {
        let mut db = self.db.lock().await;
        store::entries(&mut *db, thread, before, limit.clamp(1, 200)).await
    }

    pub(crate) async fn entries_after(
        &self,
        thread: u64,
        after: u64,
        limit: usize,
    ) -> Result<Vec<Entry>, RpcError> {
        let mut db = self.db.lock().await;
        store::entries_after(&mut *db, thread, after, limit.clamp(1, 200)).await
    }

    pub(crate) async fn run_origin(
        &self,
        thread: u64,
        run: u64,
    ) -> Result<Option<Origin>, RpcError> {
        let mut db = self.db.lock().await;
        runs::origin(&mut *db, thread, run).await
    }

    pub(crate) async fn wait_run(
        &self,
        thread: u64,
        requested: Option<u64>,
        timeout: Duration,
    ) -> Result<WaitResult, RpcError> {
        let mut changes = self.subscribe();
        let deadline = time::Instant::now() + timeout;
        let timer = time::sleep_until(deadline);
        tokio::pin!(timer);
        let mut id = requested;
        loop {
            let run = {
                let mut db = self.db.lock().await;
                let run = match id {
                    Some(id) => runs::run_from(&mut *db, thread, id).await?,
                    None => runs::latest(&mut *db, thread).await?,
                };
                drop(db);
                run
            };
            let Some(run) = run else {
                return Ok(WaitResult {
                    run: None,
                    timed_out: false,
                });
            };
            if !runs::unsettled(run.status) {
                return Ok(WaitResult {
                    run: Some(run),
                    timed_out: false,
                });
            }
            if time::Instant::now() >= deadline || self.closing.is_cancelled() {
                return Ok(WaitResult {
                    run: Some(run),
                    timed_out: true,
                });
            }
            id = Some(run.id.0);
            tokio::select! {
                () = &mut timer => {},
                () = self.closing.cancelled() => {},
                () = async {
                    loop {
                        match changes.recv().await {
                            Ok(event) if !matches!(event, Change::Settled(id) if id == thread) => {},
                            _ => break,
                        }
                    }
                } => {},
            }
        }
    }

    pub(crate) async fn attachments(
        &self,
        thread: u64,
        ids: &[String],
    ) -> Result<Vec<Attachment>, RpcError> {
        let mut db = self.db.lock().await;
        store::require(&mut *db, thread).await?;
        attachments::inputs(&mut *db, thread, ids).await
    }

    fn attachment_dir(&self, thread: u64) -> PathBuf {
        self.dir.join("attachments").join(thread.to_string())
    }

    pub(crate) async fn wake(
        self: &Arc<Self>,
        thread: u64,
        task: &str,
        text: &str,
        mode: WakeMode,
    ) -> Result<Option<u64>, RpcError> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(previous) = row(
            &mut tx,
            "SELECT run FROM task_wakes WHERE thread = ?1 AND task = ?2",
            [thread.into(), task.into()],
        )
        .await?
        {
            return previous.opt_u64(0);
        }
        if row(
            &mut tx,
            "SELECT 1 FROM threads WHERE id = ?1",
            [thread.into()],
        )
        .await?
        .is_none()
        {
            return Ok(None);
        }
        let result = NewEntry::TaskResult {
            task: task.to_owned(),
            text: text.to_owned(),
        };
        let entry = store::append(&mut tx, thread, &result).await?;
        let run = if mode == WakeMode::Prompt {
            Some(
                runs::enqueue(
                    &mut tx,
                    thread,
                    NewRun {
                        existing_entry: Some(entry),
                        ..NewRun::new(text)
                    },
                )
                .await?,
            )
        } else {
            exec(&mut tx, "UPDATE threads SET updated_at = ?2, unread = CASE WHEN ?3 THEN 1 ELSE unread END WHERE id = ?1",
                [thread.into(), store::now().into(), self.unread(thread).into()]).await?;
            None
        };
        exec(
            &mut tx,
            "INSERT INTO task_wakes (thread, task, run) VALUES (?1, ?2, ?3)",
            [thread.into(), task.into(), run.into()],
        )
        .await?;
        tx.commit().await?;
        if run.is_some() {
            self.kick(thread, false);
        }
        drop(db);
        self.relist(thread);
        Ok(run)
    }

    fn harness(&self, name: Option<&str>) -> Result<(&str, &HarnessConfig), RpcError> {
        let name = name.or(self.default_harness.as_deref()).ok_or_else(|| {
            RpcError::invalid(if self.harnesses.is_empty() {
                "no ACP harness configured"
            } else {
                "no default ACP harness configured"
            })
        })?;
        self.harnesses
            .get_key_value(name)
            .map(|(name, cfg)| (name.as_str(), cfg))
            .ok_or_else(|| RpcError::invalid(format!("ACP harness {name} is not configured")))
    }

    /// Resolve presentation from current config, not from the saved thread.
    fn harness_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.harnesses
            .get(id)
            .and_then(|harness| harness.name.as_deref())
            .unwrap_or(id)
    }

    async fn cwd_for(
        &self,
        db: &mut dyn toasty::Executor,
        project: Option<&str>,
    ) -> Result<PathBuf, RpcError> {
        match project {
            Some(project) => Ok(PathBuf::from(store::project(db, project).await?.path)),
            None => Ok(self.cwd.clone()),
        }
    }

    fn default_cwd(&self) -> &Path {
        &self.cwd
    }
}
