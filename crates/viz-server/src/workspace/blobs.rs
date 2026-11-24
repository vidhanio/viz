//! Content-addressed on-disk blob storage
//! (`$STATE/<env>/blobs/sha256/ab/<hex>`) with atomic private writes,
//! non-hardlinked read-only harness materialization, startup orphan
//! reconciliation, and background garbage collection.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex as AsyncMutex;
use viz_fs::{FsError, RelPath, Root};

use super::sql::{Connection, exec, rows};
use crate::error::Error;

const PEER_STAGE_TTL: Duration = Duration::from_secs(300);

/// On-disk content-addressed blob directory coupled with an in-flight write
/// permit map and peer-stage grace set so garbage collection never unlinks a
/// blob before its SQLite reference commits.
#[derive(Debug)]
pub(crate) struct BlobStore {
    root: PathBuf,
    in_flight: Arc<Mutex<HashMap<String, usize>>>,
    peer_staged: Arc<Mutex<HashMap<String, Instant>>>,
}

/// RAII permit keeping a freshly staged blob immune from garbage collection
/// until its SQLite transaction commits or rolls back.
#[derive(Debug)]
pub(crate) struct WritePermit {
    blob: String,
    in_flight: Arc<Mutex<HashMap<String, usize>>>,
}

impl Drop for WritePermit {
    fn drop(&mut self) {
        if let Ok(mut map) = self.in_flight.lock()
            && let Some(count) = map.get_mut(&self.blob)
        {
            if *count <= 1 {
                map.remove(&self.blob);
            } else {
                *count -= 1;
            }
        }
    }
}

/// Computes the canonical lowercase 64-character SHA-256 hex digest of `bytes`.
#[must_use]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Returns whether `id` is a valid canonical lowercase SHA-256 hex digest.
#[must_use]
pub(crate) fn is_valid_blob_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Maps a validated SHA-256 hex digest to its confined `sha256/<ab>/<hex>`
/// relative path inside the blob root.
pub(crate) fn blob_rel_path(blob_id: &str) -> Result<RelPath, Error> {
    if !is_valid_blob_id(blob_id) {
        return Err(Error::invalid("invalid blob identifier"));
    }
    RelPath::parse(&format!("sha256/{}/{}", &blob_id[..2], blob_id))
        .map_err(|error| Error::invalid(error.to_string()))
}

/// Synchronous atomic blob write used during SQLite startup migrations.
pub(crate) fn write_blob_sync(blobs_dir: &Path, bytes: &[u8]) -> anyhow::Result<String> {
    ensure_private_dir(blobs_dir)?;
    let blob_id = sha256_hex(bytes);
    let rel = RelPath::parse(&format!("sha256/{}/{}", &blob_id[..2], blob_id))?;
    let root = Root::open(blobs_dir)?;
    if !root.exists(&rel) {
        root.write_atomic_private(&rel, bytes)?;
    }
    Ok(blob_id)
}

impl BlobStore {
    /// Opens or creates the private (`0700`) blob root under `workspace_dir`.
    pub(crate) fn open(workspace_dir: &Path) -> anyhow::Result<Self> {
        let root = workspace_dir.join("blobs");
        ensure_private_dir(&root)?;
        Ok(Self {
            root,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            peer_staged: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Marks `blob_id` as recently pushed by a peer (`PUT /v2/blobs/{sha}`)
    /// so background GC preserves it during the brief window before
    /// `task.receive` links it into `thread_attachments`.
    pub(crate) fn mark_peer_staged(&self, blob_id: &str) {
        let mut staged = self
            .peer_staged
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        staged.retain(|_, when| when.elapsed() < PEER_STAGE_TTL);
        staged.insert(blob_id.to_owned(), Instant::now());
    }

    /// Acquires a GC-blocking write permit for `blob_id`.
    fn permit(&self, blob_id: &str) -> WritePermit {
        {
            let mut map = self
                .in_flight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *map.entry(blob_id.to_owned()).or_insert(0) += 1;
        }
        WritePermit {
            blob: blob_id.to_owned(),
            in_flight: Arc::clone(&self.in_flight),
        }
    }

    /// Persists `bytes` into the content-addressed store without holding any
    /// SQLite lock, returning `(blob_id, size, permit)`.
    pub(crate) async fn put(&self, bytes: Vec<u8>) -> Result<(String, u64, WritePermit), Error> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| Error::invalid("blob exceeds supported byte size"))?;
        let blob_id = sha256_hex(&bytes);
        let permit = self.permit(&blob_id);
        let root_path = self.root.clone();
        let rel = blob_rel_path(&blob_id)?;
        tokio::task::spawn_blocking(move || -> Result<(), FsError> {
            let root = Root::open(&root_path)?;
            if !root.exists(&rel) {
                root.write_atomic_private(&rel, &bytes)?;
            }
            Ok(())
        })
        .await
        .map_err(|error| Error::unavailable(format!("blob writer failed: {error}")))?
        .map_err(|error| Error::unavailable(format!("writing blob: {error}")))?;
        Ok((blob_id, size, permit))
    }

    /// Reads a byte range `[offset, offset + len)` from an immutable blob on
    /// disk without buffering the entire file in memory or holding any SQLite
    /// lock. Returns `(chunk_bytes, total_size)`.
    pub(crate) async fn read_range(
        &self,
        blob_id: &str,
        offset: u64,
        len: usize,
    ) -> Result<(Vec<u8>, u64), Error> {
        let rel = blob_rel_path(blob_id)?;
        let root_path = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let root = Root::open(&root_path)?;
            let file = root.open_regular_nofollow(&rel, false)?;
            file.read_range(offset, len)
        })
        .await
        .map_err(|error| Error::unavailable(format!("blob reader failed: {error}")))?
        .map_err(|error| match error {
            FsError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound => {
                Error::not_found(format!("blob {blob_id} is missing from disk"))
            }
            other => Error::unavailable(format!("reading blob {blob_id}: {other}")),
        })
    }

    /// Reads an immutable blob from disk with a strict byte ceiling, outside
    /// any SQLite lock.
    pub(crate) async fn read(&self, blob_id: &str, max_bytes: u64) -> Result<Vec<u8>, Error> {
        let rel = blob_rel_path(blob_id)?;
        let root_path = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let root = Root::open(&root_path)?;
            let file = root.open_regular_nofollow(&rel, false)?;
            file.read_exact_bounded(max_bytes)
        })
        .await
        .map_err(|error| Error::unavailable(format!("blob reader failed: {error}")))?
        .map_err(|error| match error {
            FsError::Io(ref io_err) if io_err.kind() == io::ErrorKind::NotFound => {
                Error::not_found(format!("blob {blob_id} is missing from disk"))
            }
            FsError::TooLarge { limit } => {
                Error::invalid(format!("blob exceeds {limit}-byte limit"))
            }
            other => Error::unavailable(format!("reading blob {blob_id}: {other}")),
        })
    }

    /// Materializes an isolated, read-only (`0400`) copy of `blob_id` inside
    /// `dest_dir` for an external harness. Canonical blobs are never
    /// hardlinked so a harness cannot mutate shared content in place.
    pub(crate) async fn materialize_copy(
        &self,
        blob_id: &str,
        dest_dir: &Path,
        dest_name: &str,
        max_bytes: u64,
    ) -> Result<PathBuf, Error> {
        let bytes = self.read(blob_id, max_bytes).await?;
        let dest_dir = dest_dir.to_owned();
        let dest_path = dest_dir.join(dest_name);
        tokio::task::spawn_blocking(move || -> io::Result<PathBuf> {
            ensure_private_dir(&dest_dir)?;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o400)
                .open(&dest_path)?;
            file.write_all(&bytes)?;
            file.flush()?;
            Ok(dest_path)
        })
        .await
        .map_err(|error| Error::unavailable(format!("materializing blob failed: {error}")))?
        .map_err(|error| Error::unavailable(format!("materializing attachment: {error}")))
    }

    /// Startup reconciliation executed before serving traffic: removes leftover
    /// `.viz-tmp-*` files and any on-disk blob files absent from SQLite
    /// `blobs`.
    pub(crate) async fn reconcile_startup(&self, db: &mut Connection) -> anyhow::Result<()> {
        let known: HashSet<String> = rows(db, "SELECT id FROM blobs", [])
            .await?
            .map(|mut row| row.str(0))
            .collect::<Result<_, _>>()?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || reconcile_dir(&root.join("sha256"), &known)).await??;
        Ok(())
    }

    /// Garbage-collects `blobs` rows no longer referenced by any row in
    /// `thread_attachments` (skipping in-flight writes), then unlinks their
    /// on-disk files outside the SQLite lock.
    pub(crate) async fn gc(&self, db_mutex: &AsyncMutex<Connection>) -> Result<usize, Error> {
        let removed: Vec<String> = {
            let mut db = db_mutex.lock().await;
            let candidates: Vec<String> = rows(
                &mut db,
                "SELECT id FROM blobs
                 WHERE NOT EXISTS (SELECT 1 FROM thread_attachments WHERE blob = blobs.id)",
                [],
            )
            .await?
            .map(|mut row| row.str(0))
            .collect::<Result<_, _>>()?;
            if candidates.is_empty() {
                return Ok(0);
            }
            let mut active: HashSet<String> = self
                .in_flight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .keys()
                .cloned()
                .collect();
            {
                let mut staged = self
                    .peer_staged
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                staged.retain(|_, when| when.elapsed() < PEER_STAGE_TTL);
                active.extend(staged.keys().cloned());
            }
            let mut tx = db.transaction().await?;
            let mut deleted = Vec::new();
            for id in candidates {
                if active.contains(&id) {
                    continue;
                }
                let count = exec(
                    &mut tx,
                    "DELETE FROM blobs
                     WHERE id = ?1
                       AND NOT EXISTS (SELECT 1 FROM thread_attachments WHERE blob = blobs.id)",
                    [id.as_str().into()],
                )
                .await?;
                if count > 0 {
                    deleted.push(id);
                }
            }
            tx.commit().await?;
            drop(db);
            deleted
        };
        if removed.is_empty() {
            return Ok(0);
        }
        let root_path = self.root.clone();
        let in_flight = Arc::clone(&self.in_flight);
        let unlinked = tokio::task::spawn_blocking(move || -> usize {
            let Ok(root) = Root::open(&root_path) else {
                return 0;
            };
            let mut count = 0usize;
            for id in removed {
                let busy = in_flight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains_key(&id);
                if busy {
                    continue;
                }
                if let Ok(rel) = blob_rel_path(&id) {
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
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

fn reconcile_dir(sha_dir: &Path, known: &HashSet<String>) -> anyhow::Result<()> {
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
