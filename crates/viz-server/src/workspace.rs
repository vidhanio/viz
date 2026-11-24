//! A local environment's ACP threads, durable run queue, project metadata, and
//! transcript.

mod admission;
mod attachments;
mod blobs;
mod checkouts;
mod checkpoints;
mod clones;
mod folders;
mod follow_ups;
mod fork;
mod git;
mod github;
mod harnesses;
mod icons;
mod import;
mod notifications;
mod projects;
mod pulls;
mod requests;
pub(crate) mod rpc;
pub(crate) mod runs;
mod runtime;
mod scratch;
mod sessions;
pub(crate) mod sql;
mod store;
mod thread;
mod triage;
mod worker;
mod worktree;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use tokio::{
    sync::{Mutex, Notify},
    time,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use viz_core::{
    placement::Checkout,
    tasks::{self, Wake, WakeMode, WakeSource},
};
use viz_protocol::{EntryId, Lifecycle, RunId, TaskId, ThreadId};

use self::{
    notifications::Updates,
    requests::Requests,
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
    blobs: blobs::BlobStore,
    /// Canonical parent of the worktrees viz creates. A checkout below it
    /// also needs a matching ownership marker before viz can remove it.
    worktrees: PathBuf,
    default_harness: Option<String>,
    harnesses: BTreeMap<String, HarnessConfig>,
    updates: Updates,
    /// Asks [`Self::keep_time`] for a new look at the clock.
    due: Notify,
    workers: Workers,
    icons: icons::Icons,
    clones: clones::Clones,
    reservations: git::Reservations,
    pulls: pulls::Pulls,
    pull_due: Notify,
    github: Mutex<BTreeMap<String, viz_github::GitHub>>,
    requests: Requests,
    /// PTYs owned by threads: the commands agents run, and later the user's
    /// own shells.
    checks: harnesses::Checks,
    mcp: Mcp,
    closing: CancellationToken,
    jobs: TaskTracker,
}

/// A task's news for its parent thread.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NewWake<'a> {
    pub(crate) thread: ThreadId,
    pub(crate) task: &'a TaskId,
    pub(crate) source: WakeSource,
    pub(crate) text: &'a str,
    pub(crate) mode: WakeMode,
}

/// What became of a wake.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Woken {
    Run(RunId),
    Entry,
    /// The thread is gone.
    Deleted,
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
        let blobs = blobs::BlobStore::open(&dir)?;
        let mut db = Connection::open(dir.join("workspace.sqlite3")).await?;
        blobs.reconcile_startup(&mut db).await?;
        let mut tx = db.transaction().await?;
        runs::recover(&mut tx).await?;
        tx.commit().await?;
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            environment: config.environment.clone(),
            cwd: config.cwd.clone(),
            dir,
            blobs,
            worktrees,
            default_harness: config.default_harness.clone(),
            harnesses: config.harnesses.clone(),
            updates: Updates::default(),
            due: Notify::new(),
            workers: Workers::default(),
            icons: icons::Icons::default(),
            clones: clones::Clones::default(),
            reservations: git::Reservations::default(),
            pulls: pulls::Pulls::default(),
            pull_due: Notify::new(),
            github: Mutex::default(),
            requests: Requests::default(),
            checks: harnesses::Checks::default(),
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
        self.jobs.spawn(Arc::clone(self).keep_time());
        self.jobs.spawn(Arc::clone(self).poll_pulls());
        self.jobs.spawn(Arc::clone(self).serve_requests());
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
        store::entries(&mut db, thread, before, limit.clamp(1, 1000)).await
    }

    /// Captures the initial transcript page, cursor head, and live baseline
    /// together.
    pub(crate) async fn watch_snapshot(
        &self,
        thread: ThreadId,
        limit: usize,
    ) -> Result<(EntryPage, Option<EntryId>, ThreadState), Error> {
        let mut db = self.db.lock().await;
        let page = store::entries(&mut db, thread, None, limit).await?;
        let last = store::latest_entry_id(&mut db, thread).await?;
        let mut state = store::state(&mut db, thread).await?;
        state.live = self.updates.latest(thread);
        drop(db);
        Ok((page, last, state))
    }

    /// Reads the next append batch and captures its baseline once caught up.
    pub(crate) async fn watch_changes(
        &self,
        thread: ThreadId,
        after: Option<EntryId>,
        limit: usize,
    ) -> Result<(Vec<Entry>, Option<ThreadState>), Error> {
        let mut db = self.db.lock().await;
        let entries = store::entries_after(&mut db, thread, after, limit).await?;
        if entries.len() == limit {
            return Ok((entries, None));
        }
        let mut state = store::state(&mut db, thread).await?;
        state.live = self.updates.latest(thread);
        drop(db);
        Ok((entries, Some(state)))
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
        attachments::load_inputs(self, thread, ids).await
    }

    pub(crate) async fn stat_blob(
        &self,
        thread: ThreadId,
        id: &str,
    ) -> Result<viz_protocol::BlobInfo, Error> {
        let mut db = self.db.lock().await;
        attachments::stat(&mut db, thread, id).await
    }

    fn attachment_dir(&self, thread: ThreadId) -> PathBuf {
        self.dir.join("attachments").join(thread.to_string())
    }

    /// Record a task's wake and whatever `then` stores with it, in one
    /// transaction. A wake already recorded is not repeated.
    pub(crate) async fn wake<T>(
        self: &Arc<Self>,
        wake: NewWake<'_>,
        then: impl AsyncFnOnce(&mut Connection, &Woken) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let woken = self.append_wake(&mut tx, &wake).await?;
        let result = then(&mut tx, &woken).await?;
        tx.commit().await?;
        if matches!(woken, Woken::Run(_)) {
            self.kick(wake.thread, false);
        }
        drop(db);
        if !matches!(woken, Woken::Deleted) {
            self.relist(wake.thread);
        }
        Ok(result)
    }

    async fn append_wake(&self, db: &mut Connection, wake: &NewWake<'_>) -> Result<Woken, Error> {
        let NewWake {
            thread,
            task,
            source,
            text,
            mode,
        } = *wake;
        let previous = row(
            db,
            "SELECT run FROM task_wakes WHERE thread = ?1 AND task = ?2 AND sequence = ?3",
            [thread.into(), task.into(), source.sequence().into()],
        )
        .await?
        .map(|row| row.opt_id(0))
        .transpose()?;
        let exists = match store::require(db, thread).await {
            Ok(()) => true,
            Err(Error::NoThread(_)) => false,
            Err(error) => return Err(error),
        };
        let mode = match tasks::wake(previous, exists, mode) {
            Wake::Replay(Some(run)) => return Ok(Woken::Run(run)),
            Wake::Replay(None) => return Ok(Woken::Entry),
            Wake::MissingThread => return Ok(Woken::Deleted),
            Wake::Append(mode) => mode,
        };
        let result = NewEntry::TaskResult {
            task: task.to_string(),
            text: text.to_owned(),
        };
        let entry = store::append(db, thread, &result).await?;
        let run = if mode == WakeMode::Prompt {
            let run = NewRun {
                existing_entry: Some(entry),
                ..NewRun::new(text)
            };
            Some(runs::enqueue(db, thread, run).await?)
        } else {
            let watched = self.is_watched(thread);
            store::touch(db, thread, |current| Lifecycle {
                unread: current.unread || !watched,
                ..current.clone()
            })
            .await?;
            None
        };
        exec(
            db,
            "INSERT INTO task_wakes (thread, task, sequence, run) VALUES (?1, ?2, ?3, ?4)",
            [
                thread.into(),
                task.into(),
                source.sequence().into(),
                run.into(),
            ],
        )
        .await?;
        Ok(run.map_or(Woken::Entry, Woken::Run))
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
