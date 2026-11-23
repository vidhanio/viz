//! A local environment's ACP threads, durable run queue, project metadata, and
//! transcript.

mod admission;
mod attachments;
mod checkouts;
mod files;
mod follow_ups;
mod notifications;
mod projects;
pub(crate) mod rpc;
pub(crate) mod runs;
mod runtime;
mod sessions;
pub(crate) mod sql;
mod store;
mod thread;
mod worker;
mod worktree;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use tokio::{sync::Mutex, time};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use viz_core::{
    placement::Checkout,
    tasks::{self, Wake, WakeMode, WakeSource},
};
use viz_protocol::{EntryId, RunId, TaskId, ThreadId};

use self::{
    files::Files,
    notifications::Updates,
    runs::NewRun,
    sql::{Connection, exec, row, rows},
    worker::Workers,
};
pub(crate) use self::{
    notifications::{Change, WatchChange},
    rpc::ROUTES,
    runs::{active_run_from, run_from},
};
use crate::{
    acp::Mcp,
    config::{Config, HarnessConfig},
    error::Error,
    wire::{Attachment, Entry, EntryPage, NewEntry, Origin, ThreadState, WaitResult},
};

pub(crate) struct Workspace {
    /// Kept for the entire process lifetime: sole writer and the physical
    /// connection shared with tasks.
    pub(crate) db: Mutex<Connection>,
    pub(crate) environment: String,
    cwd: PathBuf,
    dir: PathBuf,
    /// Canonical parent of the worktrees viz creates; any worktree below it
    /// is viz's to remove.
    worktrees: PathBuf,
    default_harness: Option<String>,
    harnesses: BTreeMap<String, HarnessConfig>,
    updates: Updates,
    workers: Workers,
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
        let worktrees = dir.join("worktrees");
        tokio::fs::create_dir_all(&worktrees)
            .await
            .with_context(|| format!("creating worktree directory {}", worktrees.display()))?;
        let worktrees = tokio::fs::canonicalize(&worktrees).await?;
        let mut db = Connection::open(dir.join("workspace.sqlite3")).await?;
        let mut tx = db.transaction().await?;
        runs::recover(&mut tx).await?;
        tx.commit().await?;
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            environment: config.environment.clone(),
            cwd: config.cwd.clone(),
            dir,
            worktrees,
            default_harness: config.default_harness.clone(),
            harnesses: config.harnesses.clone(),
            updates: Updates::default(),
            workers: Workers::default(),
            files: Files::default(),
            mcp,
            closing: CancellationToken::new(),
            jobs: TaskTracker::new(),
        }))
    }

    pub(crate) async fn resume(self: &Arc<Self>) -> Result<(), Error> {
        for row in rows(
            &mut *self.db.lock().await,
            "SELECT DISTINCT thread FROM runs WHERE status = 'queued'",
            [],
        )
        .await?
        {
            self.kick(row.id(0)?, false);
        }
        Ok(())
    }

    pub(crate) async fn state(&self, thread: ThreadId) -> Result<ThreadState, Error> {
        let mut db = self.db.lock().await;
        let mut state = store::state(&mut db, thread).await?;
        state.live = self.updates.latest(thread);
        drop(db);
        Ok(state)
    }

    pub(crate) async fn entries(
        &self,
        thread: ThreadId,
        before: Option<EntryId>,
        limit: usize,
    ) -> Result<EntryPage, Error> {
        let mut db = self.db.lock().await;
        store::entries(&mut db, thread, before, limit.clamp(1, 200)).await
    }

    pub(crate) async fn entries_after(
        &self,
        thread: ThreadId,
        after: Option<EntryId>,
        limit: usize,
    ) -> Result<Vec<Entry>, Error> {
        let mut db = self.db.lock().await;
        store::entries_after(&mut db, thread, after, limit.clamp(1, 200)).await
    }

    pub(crate) async fn run_origin(
        &self,
        thread: ThreadId,
        run: RunId,
    ) -> Result<Option<Origin>, Error> {
        let mut db = self.db.lock().await;
        runs::origin(&mut db, thread, run).await
    }

    pub(crate) async fn wait_run(
        &self,
        thread: ThreadId,
        requested: Option<RunId>,
        timeout: Duration,
    ) -> Result<WaitResult, Error> {
        let mut changes = self.subscribe();
        let deadline = time::Instant::now() + timeout;
        let timer = time::sleep_until(deadline);
        tokio::pin!(timer);
        let mut id = requested;
        loop {
            let run = {
                let mut db = self.db.lock().await;
                let run = match id {
                    Some(id) => runs::run_from(&mut db, thread, id).await?,
                    None => runs::latest(&mut db, thread).await?,
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
            if !viz_core::runs::is_open(run.status) {
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
            id = Some(run.id);
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
        thread: ThreadId,
        ids: &[String],
    ) -> Result<Vec<Attachment>, Error> {
        let mut db = self.db.lock().await;
        store::require(&mut db, thread).await?;
        attachments::inputs(&mut db, thread, ids).await
    }

    fn attachment_dir(&self, thread: ThreadId) -> PathBuf {
        self.dir.join("attachments").join(thread.to_string())
    }

    pub(crate) async fn wake(
        self: &Arc<Self>,
        thread: ThreadId,
        task: &TaskId,
        source: WakeSource,
        text: &str,
        mode: WakeMode,
    ) -> Result<Option<RunId>, Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let previous = row(
            &mut tx,
            "SELECT run FROM task_wakes WHERE thread = ?1 AND task = ?2 AND sequence = ?3",
            [thread.into(), task.into(), source.sequence().into()],
        )
        .await?
        .map(|row| row.opt_id(0))
        .transpose()?;
        let exists = if previous.is_some() {
            true
        } else {
            row(
                &mut tx,
                "SELECT 1 FROM threads WHERE id = ?1",
                [thread.into()],
            )
            .await?
            .is_some()
        };
        let mode = match tasks::wake(previous, exists, mode) {
            Wake::Replay(run) => return Ok(run),
            Wake::MissingThread => return Ok(None),
            Wake::Append(mode) => mode,
        };
        let result = NewEntry::TaskResult {
            task: task.to_string(),
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
            "INSERT INTO task_wakes (thread, task, sequence, run) VALUES (?1, ?2, ?3, ?4)",
            [
                thread.into(),
                task.into(),
                source.sequence().into(),
                run.into(),
            ],
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

    fn harness(&self, name: Option<&str>) -> Result<(&str, &HarnessConfig), Error> {
        let name = name.or(self.default_harness.as_deref()).ok_or_else(|| {
            Error::invalid(if self.harnesses.is_empty() {
                "no ACP harness configured"
            } else {
                "no default ACP harness configured"
            })
        })?;
        self.harnesses
            .get_key_value(name)
            .map(|(name, cfg)| (name.as_str(), cfg))
            .ok_or_else(|| Error::invalid(format!("ACP harness {name} is not configured")))
    }

    /// Resolve presentation from current config, not from the saved thread.
    fn harness_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.harnesses
            .get(id)
            .and_then(|harness| harness.name.as_deref())
            .unwrap_or(id)
    }

    /// The checkout of `project`, or the default directory.
    async fn checkout_for(
        &self,
        db: &mut Connection,
        project: Option<&str>,
    ) -> Result<Checkout, Error> {
        let cwd = match project {
            Some(project) => store::project(db, project).await?.path,
            None => store::path_text(&self.cwd)?.to_owned(),
        };
        Ok(Checkout {
            cwd,
            worktree: None,
        })
    }

    fn default_cwd(&self) -> &Path {
        &self.cwd
    }
}
