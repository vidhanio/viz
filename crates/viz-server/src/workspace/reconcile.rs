//! Unified convergent reconciliation for external on-disk and Git resources
//! owned by the workspace SQLite database.
//!
//! SQLite is the single source of truth for:
//! - Content-addressed blobs (`$STATE/<env>/blobs/sha256/<ab>/<hex>`)
//!   referenced by `blobs` and `thread_attachments`.
//! - Materialized read-only harness attachment directories
//!   (`$STATE/<env>/attachments/<thread>`), owned by `threads.id`.
//! - Projectless scratch directories (`$STATE/<env>/scratch/<dir>`), owned by
//!   `threads.cwd` or promoted `projects.path`.
//! - Deleted-thread Git checkpoint refs queued in `checkpoint_releases`.
//!
//! Managed Git worktrees are intentionally excluded from automatic sweeping so
//! a worktree kept after thread deletion or holding uncommitted user edits is
//! never discarded without an explicit user action.
//!
//! [`ResourceLeases`] protects in-flight creations (blobs being staged, peer
//! blobs awaiting `task.receive`, and scratch directories prepared outside the
//! database lock) so periodic reconciliation can never race a committing
//! operation.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use viz_fs::{FsError, Root};

use super::{
    Workspace, blobs,
    sql::{Database, exec, rows},
};
use crate::error::Error;

const PEER_STAGE_TTL: Duration = Duration::from_secs(300);
const CLIENT_STAGE_TTL: Duration = Duration::from_secs(600);

/// In-memory lease registry preventing background reconciliation from removing
/// external resources whose owning SQLite rows have not committed yet.
#[derive(Debug, Default)]
pub(crate) struct ResourceLeases {
    blobs: Mutex<HashMap<String, usize>>,
    peer_staged_blobs: Mutex<HashMap<String, Instant>>,
    client_staged_blobs: Mutex<HashMap<(String, String), Instant>>,
    scratch_dirs: Mutex<HashMap<PathBuf, usize>>,
}

/// RAII lease keeping a staged blob immune from garbage collection until its
/// referencing SQLite transaction commits or rolls back.
#[derive(Debug)]
pub(crate) struct BlobPermit {
    blob: String,
    leases: Arc<ResourceLeases>,
}

impl Drop for BlobPermit {
    fn drop(&mut self) {
        let mut map = self
            .leases
            .blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = map.get_mut(&self.blob) {
            if *count <= 1 {
                map.remove(&self.blob);
            } else {
                *count -= 1;
            }
        }
        drop(map);
    }
}

/// RAII lease keeping a newly created scratch directory immune from background
/// orphan sweeping until its thread commits to SQLite or is discarded.
#[derive(Debug)]
pub(crate) struct ScratchPermit {
    path: PathBuf,
    leases: Arc<ResourceLeases>,
}

impl Drop for ScratchPermit {
    fn drop(&mut self) {
        let mut map = self
            .leases
            .scratch_dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = map.get_mut(&self.path) {
            if *count <= 1 {
                map.remove(&self.path);
            } else {
                *count -= 1;
            }
        }
        drop(map);
    }
}

impl ResourceLeases {
    /// Acquires a GC-blocking write permit for `blob_id`.
    pub(crate) fn lease_blob(self: &Arc<Self>, blob_id: &str) -> BlobPermit {
        {
            let mut map = self
                .blobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *map.entry(blob_id.to_owned()).or_insert(0) += 1;
        }
        BlobPermit {
            blob: blob_id.to_owned(),
            leases: Arc::clone(self),
        }
    }

    /// Marks `blob_id` as recently pushed by a peer (`PUT /v2/blobs/{sha}`)
    /// so background GC preserves it while `task.receive` links it.
    pub(crate) fn mark_peer_staged(&self, blob_id: &str) {
        let mut staged = self
            .peer_staged_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, when| when.elapsed() < PEER_STAGE_TTL);
        staged.insert(blob_id.to_owned(), Instant::now());
    }

    /// Marks `blob_id` as staged by an authenticated client principal via the
    /// content WebSocket (`UploadStage`), keeping it alive for
    /// `CLIENT_STAGE_TTL` until claimed by `thread.launch`, `thread.send`,
    /// `task.delegate`, or `project.update`.
    pub(crate) fn mark_client_staged(&self, principal_key: &str, blob_id: &str) {
        let mut staged = self
            .client_staged_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, when| when.elapsed() < CLIENT_STAGE_TTL);
        staged.insert(
            (principal_key.to_owned(), blob_id.to_owned()),
            Instant::now(),
        );
    }

    /// Returns whether `principal_key` holds an unexpired content-plane stage
    /// lease for `blob_id`.
    pub(crate) fn has_client_stage(&self, principal_key: &str, blob_id: &str) -> bool {
        let mut staged = self
            .client_staged_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, when| when.elapsed() < CLIENT_STAGE_TTL);
        staged.contains_key(&(principal_key.to_owned(), blob_id.to_owned()))
    }

    /// Returns every blob identifier currently protected by an active write
    /// permit or unexpired stage lease.
    pub(crate) fn active_blobs(&self) -> HashSet<String> {
        let mut active: HashSet<String> = self
            .blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        {
            let mut staged = self
                .peer_staged_blobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            staged.retain(|_, when| when.elapsed() < PEER_STAGE_TTL);
            active.extend(staged.keys().cloned());
        }
        {
            let mut staged = self
                .client_staged_blobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            staged.retain(|_, when| when.elapsed() < CLIENT_STAGE_TTL);
            active.extend(staged.keys().map(|(_, blob)| blob.clone()));
        }
        active
    }

    /// Returns whether `blob_id` currently holds an in-flight write permit or
    /// unexpired stage lease.
    pub(crate) fn is_blob_active(&self, blob_id: &str) -> bool {
        let in_flight = self
            .blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(blob_id);
        if in_flight {
            return true;
        }
        {
            let mut staged = self
                .peer_staged_blobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            staged.retain(|_, when| when.elapsed() < PEER_STAGE_TTL);
            if staged.contains_key(blob_id) {
                return true;
            }
        }
        let mut staged = self
            .client_staged_blobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, when| when.elapsed() < CLIENT_STAGE_TTL);
        staged.keys().any(|(_, blob)| blob == blob_id)
    }

    /// Acquires an orphan-sweep lease for `path` before creating the scratch
    /// directory on disk.
    pub(crate) fn lease_scratch(self: &Arc<Self>, path: PathBuf) -> ScratchPermit {
        {
            let mut map = self
                .scratch_dirs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *map.entry(path.clone()).or_insert(0) += 1;
        }
        ScratchPermit {
            path,
            leases: Arc::clone(self),
        }
    }

    /// Returns whether `path` is currently leased by an in-flight scratch
    /// creation.
    pub(crate) fn is_scratch_leased(&self, path: &Path) -> bool {
        self.scratch_dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(path)
    }
}

/// Startup reconciliation executed before serving traffic: removes leftover
/// temp files, orphaned blob files absent from `blobs`, materialized
/// attachment directories for deleted threads, and orphaned scratch
/// directories not referenced by any thread or promoted project.
pub(super) async fn reconcile_startup(
    dir: &Path,
    blobs_root: &Path,
    db: &Database,
) -> anyhow::Result<()> {
    let (known_blobs, live_threads, live_paths) = db
        .read_snapshot(|tx| {
            let known_blobs: HashSet<String> = rows(tx, "SELECT id FROM blobs", [])?
                .map(|mut row| row.str(0))
                .collect::<Result<_, _>>()?;
            let live_threads: HashSet<String> = rows(tx, "SELECT id FROM threads", [])?
                .map(|row| Ok(row.u64(0)?.to_string()))
                .collect::<Result<_, Error>>()?;
            let mut live_paths = HashSet::new();
            for mut row in rows(
                tx,
                "SELECT cwd FROM threads UNION SELECT path FROM projects",
                [],
            )? {
                live_paths.insert(PathBuf::from(row.str(0)?));
            }
            Ok((known_blobs, live_threads, live_paths))
        })
        .await?;
    let dir = dir.to_owned();
    let blobs_root = blobs_root.to_owned();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        reconcile_blob_dir(&blobs_root.join("sha256"), &known_blobs)?;
        sweep_upload_temps(&blobs_root.join("uploads"));
        sweep_attachment_dirs(&dir.join("attachments"), &live_threads);
        sweep_scratch_dirs(&dir.join("scratch"), &live_paths);
        Ok(())
    })
    .await??;
    Ok(())
}

impl Workspace {
    /// Runs every convergent external-resource cleanup pass in one cycle:
    /// deleted-thread checkpoint refs, unreferenced blobs, orphaned
    /// materialized attachment directories, and orphaned scratch directories.
    pub(super) async fn reconcile_periodic(&self) {
        if let Err(error) = self.release_checkpoints().await {
            tracing::warn!(%error, "releasing deleted thread checkpoints");
        }
        if let Err(error) = self.gc_blobs().await {
            tracing::warn!(%error, "collecting unreferenced attachment blobs");
        }
        if let Err(error) = self.sweep_orphan_directories().await {
            tracing::warn!(%error, "sweeping orphaned workspace directories");
        }
    }

    async fn gc_blobs(&self) -> Result<usize, Error> {
        let leases = Arc::clone(&self.leases);
        let removed: Vec<String> = self
            .db
            .write_tx(move |tx| {
                let active = leases.active_blobs();
                let mut deleted = Vec::new();
                for _ in 0..2 {
                    let candidates: Vec<String> = rows(
                        tx,
                        "SELECT id FROM blobs
                         WHERE NOT EXISTS (SELECT 1 FROM thread_attachments WHERE blob = blobs.id)
                           AND NOT EXISTS (SELECT 1 FROM blobs AS parent WHERE parent.preview = blobs.id)
                           AND NOT EXISTS (SELECT 1 FROM projects WHERE icon_blob = blobs.id OR json_extract(settings, '$.icon.blob') = blobs.id)",
                        [],
                    )?
                    .map(|mut row| row.str(0))
                    .collect::<Result<_, _>>()?;
                    if candidates.is_empty() {
                        break;
                    }
                    let mut pass_deleted = 0usize;
                    for id in candidates {
                        if active.contains(&id) {
                            continue;
                        }
                        let count = exec(
                            tx,
                            "DELETE FROM blobs
                             WHERE id = ?1
                               AND NOT EXISTS (SELECT 1 FROM thread_attachments WHERE blob = blobs.id)
                               AND NOT EXISTS (SELECT 1 FROM blobs AS parent WHERE parent.preview = blobs.id)
                               AND NOT EXISTS (SELECT 1 FROM projects WHERE icon_blob = blobs.id OR json_extract(settings, '$.icon.blob') = blobs.id)",
                            [id.as_str().into()],
                        )?;
                        if count > 0 {
                            deleted.push(id);
                            pass_deleted += 1;
                        }
                    }
                    if pass_deleted == 0 {
                        break;
                    }
                }
                Ok(deleted)
            })
            .await?;
        if removed.is_empty() {
            return Ok(0);
        }
        let root_path = self.blobs.root().to_owned();
        let leases = Arc::clone(&self.leases);
        let unlinked = tokio::task::spawn_blocking(move || -> usize {
            let Ok(root) = Root::open(&root_path) else {
                return 0;
            };
            let mut count = 0usize;
            for id in removed {
                if leases.is_blob_active(&id) {
                    continue;
                }
                if let Ok(rel) = blobs::blob_rel_path(&id) {
                    match root.remove_file(&rel) {
                        Ok(()) => count += 1,
                        Err(FsError::Io(ref err)) if err.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => {
                            tracing::warn!(%id, %error, "unlinking garbage-collected blob");
                        }
                    }
                }
            }
            count
        })
        .await
        .unwrap_or(0);
        Ok(unlinked)
    }

    /// Reconciles `attachments/<thread>` and `scratch/<dir>` against SQLite.
    ///
    /// Linearizability proof against concurrent creation:
    /// 1. List disk entries first.
    /// 2. Check in-memory leases (`ScratchPermit` is acquired before
    ///    `create_dir` and dropped only after SQLite commit).
    /// 3. Query SQLite `live_threads` and `live_paths`.
    ///
    /// Any directory that was not leased in step 2 either already committed
    /// before step 3 (so step 3 observes it) or was truly orphaned.
    async fn sweep_orphan_directories(&self) -> Result<(), Error> {
        let attachments_root = self.dir.join("attachments");
        let scratch_root = self.dir.join("scratch");
        let leases = Arc::clone(&self.leases);
        let (attachment_candidates, scratch_candidates) = tokio::task::spawn_blocking(move || {
            let attachments = list_child_entries(&attachments_root);
            let scratch: Vec<PathBuf> = list_child_entries(&scratch_root)
                .into_iter()
                .map(|(_, path)| path)
                .filter(|path| !leases.is_scratch_leased(path))
                .collect();
            (attachments, scratch)
        })
        .await
        .map_err(|error| Error::unavailable(format!("orphan scan failed: {error}")))?;
        if attachment_candidates.is_empty() && scratch_candidates.is_empty() {
            return Ok(());
        }
        let (live_threads, live_paths) = self
            .db
            .read_snapshot(|tx| {
                let live_threads: HashSet<String> = rows(tx, "SELECT id FROM threads", [])?
                    .map(|row| Ok(row.u64(0)?.to_string()))
                    .collect::<Result<_, Error>>()?;
                let mut live_paths = HashSet::new();
                for mut row in rows(
                    tx,
                    "SELECT cwd FROM threads UNION SELECT path FROM projects",
                    [],
                )? {
                    live_paths.insert(PathBuf::from(row.str(0)?));
                }
                Ok((live_threads, live_paths))
            })
            .await?;
        let leases = Arc::clone(&self.leases);
        tokio::task::spawn_blocking(move || {
            for (name, path) in attachment_candidates {
                if !live_threads.contains(&name) {
                    remove_path(&path, "orphaned attachment directory");
                }
            }
            for path in scratch_candidates {
                let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if !live_paths.contains(&path)
                    && !live_paths.contains(&canonical)
                    && !leases.is_scratch_leased(&path)
                    && !leases.is_scratch_leased(&canonical)
                {
                    remove_path(&path, "orphaned scratch directory");
                }
            }
        })
        .await
        .map_err(|error| Error::unavailable(format!("orphan sweep failed: {error}")))?;
        Ok(())
    }
}

fn reconcile_blob_dir(sha_dir: &Path, known: &HashSet<String>) -> anyhow::Result<()> {
    let entries = match fs::read_dir(sha_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", sha_dir.display()));
        }
    };
    for shard in entries {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(shard.path())? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if name_str.starts_with(".viz-tmp-") || !known.contains(name_str) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

fn sweep_upload_temps(root: &Path) {
    for (_, path) in list_child_entries(root) {
        remove_path(&path, "orphaned startup upload temporary file");
    }
}

fn sweep_attachment_dirs(root: &Path, live_threads: &HashSet<String>) {
    for (name, path) in list_child_entries(root) {
        if !live_threads.contains(&name) {
            remove_path(&path, "orphaned startup attachment directory");
        }
    }
}

fn sweep_scratch_dirs(root: &Path, live_paths: &HashSet<PathBuf>) {
    for (_, path) in list_child_entries(root) {
        let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !live_paths.contains(&path) && !live_paths.contains(&canonical) {
            remove_path(&path, "orphaned startup scratch directory");
        }
    }
}

fn list_child_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            out.push((name, entry.path()));
        }
    }
    out
}

fn remove_path(path: &Path, label: &str) {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "removing {label}");
    }
}
