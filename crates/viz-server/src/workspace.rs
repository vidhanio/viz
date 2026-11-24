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
mod reconcile;
mod requests;
pub(crate) mod rpc;
pub(crate) mod runs;
mod runtime;
mod scratch;
mod sessions;
pub(crate) mod sql;
mod store;
mod thread;
mod thumbs;
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
    reconcile::ResourceLeases,
    requests::Requests,
    runs::NewRun,
    sql::{Conn, Database, exec, row, rows},
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
    /// Concurrent WAL reader pool paired with a serialized synchronous SQLite
    /// writer thread.
    pub(crate) db: Database,
    pub(crate) environment: String,
    cwd: PathBuf,
    dir: PathBuf,
    leases: Arc<ResourceLeases>,
    blobs: blobs::BlobStore,
    thumbs: thumbs::ThumbCache,
    entry_chunks: Arc<store::EntryChunkCache>,
    /// Canonical parent of the worktrees viz creates. A checkout below it
    /// also needs a matching ownership marker before viz can remove it.
    worktrees: PathBuf,
    default_harness: Option<String>,
    harnesses: BTreeMap<String, HarnessConfig>,
    updates: Arc<Updates>,
    /// Asks [`Self::keep_time`] for a new look at the clock.
    due: Notify,
    workers: Arc<Workers>,
    icons: icons::Icons,
    clones: clones::Clones,
    reservations: Arc<git::Reservations>,
    pulls: pulls::Pulls,
    pull_due: Notify,
    github: Mutex<BTreeMap<String, viz_github::GitHub>>,
    requests: Arc<Requests>,
    /// PTYs owned by threads: the commands agents run, and later the user's
    /// own shells.
    checks: harnesses::Checks,
    mcp: Mcp,
    closing: CancellationToken,
    jobs: TaskTracker,
}

/// A task's news for its parent thread.
#[derive(Debug, Clone)]
pub(crate) struct NewWake {
    pub(crate) thread: ThreadId,
    pub(crate) task: TaskId,
    pub(crate) source: WakeSource,
    pub(crate) text: String,
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
        let cwd = tokio::fs::canonicalize(&config.cwd)
            .await
            .unwrap_or_else(|_| config.cwd.clone());
        let leases = Arc::new(ResourceLeases::default());
        let blobs = blobs::BlobStore::open(&dir, Arc::clone(&leases))?;
        let db = Database::open(dir.join("workspace.sqlite3")).await?;
        reconcile::reconcile_startup(&dir, blobs.root(), &db).await?;
        db.write_tx(|tx| runs::recover(tx)).await?;
        Ok(Arc::new(Self {
            db,
            environment: config.environment.clone(),
            cwd,
            dir,
            leases,
            blobs,
            thumbs: thumbs::ThumbCache::default(),
            entry_chunks: Arc::new(store::EntryChunkCache::default()),
            worktrees,
            default_harness: config.default_harness.clone(),
            harnesses: config.harnesses.clone(),
            updates: Arc::new(Updates::default()),
            due: Notify::new(),
            workers: Arc::new(Workers::default()),
            icons: icons::Icons::default(),
            clones: clones::Clones::default(),
            reservations: Arc::new(git::Reservations::default()),
            pulls: pulls::Pulls::default(),
            pull_due: Notify::new(),
            github: Mutex::default(),
            requests: Arc::new(Requests::default()),
            checks: harnesses::Checks::default(),
            mcp,
            closing: CancellationToken::new(),
            jobs: TaskTracker::new(),
        }))
    }

    pub(crate) async fn resume(self: &Arc<Self>) -> Result<(), Error> {
        let queued: Vec<ThreadId> = self
            .db
            .read(|conn| {
                rows(
                    conn,
                    "SELECT DISTINCT thread FROM runs WHERE status = 'queued'",
                    [],
                )?
                .map(|row| row.id(0))
                .collect()
            })
            .await?;
        for thread in queued {
            self.kick(thread, false);
        }
        self.jobs.spawn(Arc::clone(self).keep_time());
        self.jobs.spawn(Arc::clone(self).poll_pulls());
        self.jobs.spawn(Arc::clone(self).serve_requests());
        Ok(())
    }

    pub(crate) async fn list_projects(&self) -> Result<Vec<crate::wire::Project>, Error> {
        let rows = self.db.read(|conn| store::project_rows(conn)).await?;
        let mut projects: Vec<_> = futures_util::future::join_all(
            rows.into_iter()
                .map(|row| store::project_at(row.path, row.name, row.created_at, row.settings)),
        )
        .await;
        projects.sort_by_key(|project| {
            (
                !project.settings.pinned,
                project.settings.order.unwrap_or(i64::MAX),
            )
        });
        Ok(projects)
    }

    pub(crate) async fn read_project(&self, path: &str) -> Result<crate::wire::Project, Error> {
        let path = path.to_owned();
        let row = self
            .db
            .read(move |conn| store::project_row(conn, &path))
            .await?;
        Ok(store::project_at(row.path, row.name, row.created_at, row.settings).await)
    }

    pub(crate) async fn list_threads(
        &self,
        archived: Option<bool>,
        project: Option<&str>,
    ) -> Result<Vec<crate::wire::ThreadSummary>, Error> {
        let project = project.map(str::to_owned);
        let records = self
            .db
            .read_snapshot(move |tx| store::thread_rows(tx, archived, project.as_deref()))
            .await?;
        let mut unique_worktrees: Vec<String> = records
            .iter()
            .filter_map(|record| record.worktree_path.clone())
            .collect();
        unique_worktrees.sort_unstable();
        unique_worktrees.dedup();
        let resolved: std::collections::HashMap<String, crate::wire::ThreadWorktree> =
            futures_util::future::join_all(unique_worktrees.into_iter().map(|path| async move {
                store::thread_worktree(Some(path.clone()))
                    .await
                    .map(|wt| (path, wt))
            }))
            .await
            .into_iter()
            .flatten()
            .collect();
        let mut threads: Vec<_> = records
            .into_iter()
            .map(|mut record| {
                record.summary.pull_request = self.pull_summary(record.summary.id, &record.cwd);
                record.summary.harness_name = self.harness_name(&record.summary.harness).to_owned();
                if let Some(path) = record.worktree_path.as_deref() {
                    record.summary.worktree = resolved.get(path).cloned();
                }
                (record.summary, record.activity)
            })
            .collect();
        threads.sort_by_cached_key(|(thread, activity)| {
            viz_core::lifecycle::sort_key(thread.shelf, &thread.lifecycle, activity, thread.id)
        });
        Ok(threads.into_iter().map(|(thread, _)| thread).collect())
    }

    pub(crate) async fn state(&self, thread: ThreadId) -> Result<ThreadState, Error> {
        let updates = Arc::clone(&self.updates);
        let (mut state, worktree_path) = self
            .db
            .read_snapshot(move |tx| {
                let (mut state, worktree_path) = store::state_row(tx, thread)?;
                state.live = updates.latest(thread);
                Ok((state, worktree_path))
            })
            .await?;
        state.worktree = store::thread_worktree(worktree_path).await;
        Ok(state)
    }

    pub(crate) async fn entries(
        &self,
        thread: ThreadId,
        before: Option<EntryId>,
        limit: usize,
    ) -> Result<EntryPage, Error> {
        let limit = limit.clamp(1, 1000);
        self.db
            .read_snapshot(move |tx| store::entries(tx, thread, before, limit))
            .await
    }

    /// Captures the initial transcript page, cursor head, and live baseline
    /// together in one WAL read snapshot, resolving worktree branch metadata
    /// outside SQLite.
    pub(crate) async fn watch_snapshot(
        &self,
        thread: ThreadId,
        limit: usize,
    ) -> Result<(EntryPage, Option<EntryId>, ThreadState), Error> {
        let updates = Arc::clone(&self.updates);
        let (page, last, mut state, worktree_path) = self
            .db
            .read_snapshot(move |tx| {
                let page = store::entries(tx, thread, None, limit)?;
                let last = store::latest_entry_id(tx, thread)?;
                let (mut state, worktree_path) = store::state_row(tx, thread)?;
                state.live = updates.latest(thread);
                Ok((page, last, state, worktree_path))
            })
            .await?;
        state.worktree = store::thread_worktree(worktree_path).await;
        Ok((page, last, state))
    }

    /// Reads the next append batch and captures its baseline once caught up,
    /// resolving worktree branch metadata outside SQLite.
    pub(crate) async fn watch_changes(
        &self,
        thread: ThreadId,
        after: Option<EntryId>,
        limit: usize,
    ) -> Result<(Vec<Entry>, Option<ThreadState>), Error> {
        let updates = Arc::clone(&self.updates);
        let (entries, state) = self
            .db
            .read_snapshot(move |tx| {
                let (entries, has_more) = store::entries_after(tx, thread, after, limit)?;
                if has_more {
                    return Ok((entries, None));
                }
                let (mut state, worktree_path) = store::state_row(tx, thread)?;
                state.live = updates.latest(thread);
                Ok((entries, Some((state, worktree_path))))
            })
            .await?;
        let state = match state {
            Some((mut state, worktree_path)) => {
                state.worktree = store::thread_worktree(worktree_path).await;
                Some(state)
            }
            None => None,
        };
        Ok((entries, state))
    }

    pub(crate) async fn run_origin(
        &self,
        thread: ThreadId,
        run: RunId,
    ) -> Result<Option<Origin>, Error> {
        self.db
            .read(move |conn| runs::origin(conn, thread, run))
            .await
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
            let run = self
                .db
                .read(move |conn| {
                    id.map_or_else(
                        || runs::latest(conn, thread),
                        |id| runs::run_from(conn, thread, id),
                    )
                })
                .await?;
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
        let id = id.to_owned();
        self.db
            .read(move |conn| attachments::stat(conn, thread, &id))
            .await
    }

    pub(crate) async fn read_blob(&self, blob_id: &str, max_bytes: u64) -> Result<Vec<u8>, Error> {
        self.blobs
            .read(
                blob_id,
                max_bytes.min(viz_protocol::limits::ATTACHMENT_TOTAL_BYTES as u64),
            )
            .await
    }

    /// Validates and persists a blob pushed by an authenticated peer
    /// (`PUT /v2/blobs/{sha}`), marking it staged for GC until `task.receive`
    /// attaches it to a thread.
    pub(crate) async fn receive_peer_blob(
        &self,
        expected_sha: &str,
        bytes: Vec<u8>,
    ) -> Result<(), Error> {
        if !blobs::is_valid_blob_id(expected_sha) {
            return Err(Error::invalid("invalid blob sha256"));
        }
        if bytes.len() > viz_protocol::limits::ATTACHMENT_TOTAL_BYTES {
            return Err(Error::invalid("blob exceeds 20 MiB limit"));
        }
        let actual = blobs::sha256_hex(&bytes);
        if actual != expected_sha {
            return Err(Error::invalid("blob digest does not match path sha256"));
        }
        let (blob_id, size, permit) = self.blobs.put(bytes).await?;
        let now = store::now();
        let blob_for_write = blob_id.clone();
        self.db
            .write(move |conn| {
                exec(
                    conn,
                    "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT DO NOTHING",
                    [
                        blob_for_write.as_str().into(),
                        size.into(),
                        "application/octet-stream".into(),
                        now.into(),
                    ],
                )?;
                Ok(())
            })
            .await?;
        self.blobs.mark_peer_staged(&blob_id);
        drop(permit);
        Ok(())
    }

    pub(crate) async fn prepare_delegated_attachments(
        &self,
        parent: ThreadId,
        uploads: Vec<Attachment>,
        stored_ids: &[String],
    ) -> Result<Vec<viz_protocol::peer::PeerAttachment>, Error> {
        attachments::prepare_delegated(self, parent, uploads, stored_ids).await
    }

    pub(crate) async fn store_uploaded_attachment(
        &self,
        thread: ThreadId,
        name: String,
        mime: String,
        bytes: Vec<u8>,
    ) -> Result<viz_protocol::BlobInfo, Error> {
        attachments::store_raw(self, thread, name, mime, bytes).await
    }

    /// Reads one content-plane chunk for a thread-owned attachment, either raw
    /// bytes or a cached server-rendered preview thumbnail.
    pub(crate) async fn read_attachment_chunk(
        &self,
        thread: ThreadId,
        attachment: &str,
        variant: viz_protocol::ContentVariant,
        offset: u64,
        len: usize,
    ) -> Result<(Vec<u8>, u64, String), Error> {
        let info = self.stat_blob(thread, attachment).await?;
        match variant {
            viz_protocol::ContentVariant::Original => {
                let (bytes, total) = self.blobs.read_range(&info.blob, offset, len).await?;
                Ok((bytes, total, info.mime))
            }
            viz_protocol::ContentVariant::Preview { max_px } => {
                let rendered = if let Some(hit) = self.thumbs.get(&info.blob, max_px) {
                    hit
                } else {
                    let raw = self
                        .blobs
                        .read(
                            &info.blob,
                            info.size
                                .min(viz_protocol::limits::ATTACHMENT_TOTAL_BYTES as u64),
                        )
                        .await?;
                    self.thumbs
                        .render(info.blob, info.mime, raw, max_px)
                        .await?
                };
                Ok(slice_rendered(&rendered, offset, len))
            }
        }
    }

    /// Reads one content-plane chunk for a file confined inside a thread or
    /// project root, either raw bytes or a cached server-rendered preview
    /// thumbnail.
    pub(crate) async fn read_file_chunk(
        &self,
        root: viz_protocol::FileRoot,
        path: &str,
        variant: viz_protocol::ContentVariant,
        offset: u64,
        len: usize,
    ) -> Result<(Vec<u8>, u64, String), Error> {
        let rel =
            viz_fs::RelPath::parse(path).map_err(|error| Error::invalid(error.to_string()))?;
        let root_dir = match root {
            viz_protocol::FileRoot::Thread { thread } => {
                self.db
                    .read(move |conn| store::thread_cwd(conn, thread))
                    .await?
            }
            viz_protocol::FileRoot::Project { project } => {
                self.db
                    .read(move |conn| store::require_project(conn, &project))
                    .await?
            }
        };
        let root_path = PathBuf::from(root_dir);
        match variant {
            viz_protocol::ContentVariant::Original => tokio::task::spawn_blocking(move || {
                let root = viz_fs::Root::open(&root_path)
                    .map_err(|error| Error::invalid(format!("couldn't open root: {error}")))?;
                let file = root
                    .open_regular(&rel)
                    .map_err(|error| Error::invalid(format!("couldn't open file: {error}")))?;
                let (bytes, total) = file
                    .read_range(offset, len)
                    .map_err(|error| Error::invalid(format!("couldn't read file: {error}")))?;
                let mime = if offset == 0 {
                    viz_fs::sniff_mime(rel.as_path(), &bytes)
                } else {
                    mime_guess::from_path(rel.as_path())
                        .first_raw()
                        .unwrap_or("application/octet-stream")
                        .to_owned()
                };
                Ok((bytes, total, mime))
            })
            .await
            .map_err(|error| Error::unavailable(format!("file reader failed: {error}")))?,
            viz_protocol::ContentVariant::Preview { max_px } => {
                let (raw, mime) = tokio::task::spawn_blocking(move || {
                    let root = viz_fs::Root::open(&root_path)
                        .map_err(|error| Error::invalid(format!("couldn't open root: {error}")))?;
                    let file = root
                        .open_regular(&rel)
                        .map_err(|error| Error::invalid(format!("couldn't open file: {error}")))?;
                    let raw = file
                        .read_exact_bounded(viz_protocol::limits::PREVIEW_IMAGE_BYTES as u64)
                        .map_err(|error| Error::invalid(format!("couldn't read image: {error}")))?;
                    let mime = viz_fs::sniff_mime(rel.as_path(), &raw);
                    Ok::<_, Error>((raw, mime))
                })
                .await
                .map_err(|error| Error::unavailable(format!("file reader failed: {error}")))??;
                let key = blobs::sha256_hex(&raw);
                let rendered = self.thumbs.render(key, mime, raw, max_px).await?;
                Ok(slice_rendered(&rendered, offset, len))
            }
        }
    }

    fn attachment_dir(&self, thread: ThreadId) -> PathBuf {
        self.dir.join("attachments").join(thread.to_string())
    }

    /// Record a task's wake and whatever `then` stores with it, in one
    /// synchronous write transaction. A wake already recorded is not repeated.
    pub(crate) async fn wake<T: Send + 'static>(
        self: &Arc<Self>,
        wake: NewWake,
        then: impl FnOnce(&Conn, &Woken) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let thread = wake.thread;
        let ws = Arc::clone(self);
        let (result, woken) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                let woken = ws.append_wake(&tx, &wake)?;
                let result = then(&tx, &woken)?;
                tx.commit()?;
                if matches!(woken, Woken::Run(_)) {
                    ws.kick(wake.thread, false);
                }
                Ok((result, woken))
            })
            .await?;
        if !matches!(woken, Woken::Deleted) {
            self.relist(thread);
        }
        Ok(result)
    }

    fn append_wake(&self, db: &Conn, wake: &NewWake) -> Result<Woken, Error> {
        let NewWake {
            thread,
            ref task,
            source,
            ref text,
            mode,
        } = *wake;
        let previous = row(
            db,
            "SELECT run FROM task_wakes WHERE thread = ?1 AND task = ?2 AND sequence = ?3",
            [thread.into(), task.into(), source.sequence().into()],
        )?
        .map(|row| row.opt_id(0))
        .transpose()?;
        let exists = match store::require(db, thread) {
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
            text: text.clone(),
        };
        let entry = store::append(db, thread, &result)?;
        let run = if mode == WakeMode::Prompt {
            let run = NewRun {
                existing_entry: Some(entry),
                ..NewRun::new(text)
            };
            Some(runs::enqueue(db, thread, run)?)
        } else {
            let watched = self.is_watched(thread);
            store::touch(db, thread, |current| Lifecycle {
                unread: current.unread || !watched,
                ..current.clone()
            })?;
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
        )?;
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

    /// The checkout of `project`, or the default directory (pure SQLite check;
    /// zero Git or filesystem I/O).
    fn checkout_for(&self, db: &Conn, project: Option<&str>) -> Result<Checkout, Error> {
        let cwd = match project {
            Some(project) => store::require_project(db, project)?,
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

fn slice_rendered(
    rendered: &thumbs::RenderedImage,
    offset: u64,
    len: usize,
) -> (Vec<u8>, u64, String) {
    let total = rendered.bytes.len() as u64;
    let start = usize::try_from(offset)
        .unwrap_or(rendered.bytes.len())
        .min(rendered.bytes.len());
    let end = start.saturating_add(len).min(rendered.bytes.len());
    (
        rendered.bytes[start..end].to_vec(),
        total,
        rendered.mime.clone(),
    )
}
