//! Content-addressed on-disk blob storage
//! (`$STATE/<env>/blobs/sha256/ab/<hex>`) with atomic private writes and
//! non-hardlinked read-only harness materialization.

use std::{
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use sha2::{Digest as _, Sha256};
use viz_fs::{FsError, RelPath, Root};

use super::reconcile::{BlobPermit, ResourceLeases};
use crate::error::Error;

/// On-disk content-addressed blob directory backed by [`ResourceLeases`] so
/// background garbage collection never unlinks a blob before its SQLite
/// reference commits.
#[derive(Debug)]
pub(crate) struct BlobStore {
    root: PathBuf,
    leases: Arc<ResourceLeases>,
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
    pub(crate) fn open(workspace_dir: &Path, leases: Arc<ResourceLeases>) -> anyhow::Result<Self> {
        let root = workspace_dir.join("blobs");
        ensure_private_dir(&root)?;
        Ok(Self { root, leases })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Marks `blob_id` as recently pushed by a peer (`PUT /v2/blobs/{sha}`)
    /// so background GC preserves it while `task.receive` links it.
    pub(crate) fn mark_peer_staged(&self, blob_id: &str) {
        self.leases.mark_peer_staged(blob_id);
    }

    /// Persists `bytes` into the content-addressed store without holding any
    /// SQLite lock, returning `(blob_id, size, permit)`.
    pub(crate) async fn put(&self, bytes: Vec<u8>) -> Result<(String, u64, BlobPermit), Error> {
        let size = u64::try_from(bytes.len())
            .map_err(|_| Error::invalid("blob exceeds supported byte size"))?;
        let blob_id = sha256_hex(&bytes);
        let permit = self.leases.lease_blob(&blob_id);
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
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}
