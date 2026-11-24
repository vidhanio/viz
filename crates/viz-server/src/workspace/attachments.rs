//! Attachment decoding, staging into [`super::blobs::BlobStore`], thread
//! membership checks, unified prompt byte budgeting, and read-only harness
//! materialization.

use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use viz_protocol::{BlobInfo, RunId, ThreadId, peer::PeerAttachment};

use super::{
    Workspace,
    reconcile::BlobPermit,
    sql::{Conn, exec, row},
    store,
};
use crate::{
    error::Error,
    wire::{Attachment, PromptAttachment, StoredAttachment},
};

const TOTAL_LIMIT: usize = viz_protocol::limits::ATTACHMENT_TOTAL_BYTES;

/// Attachment bytes already persisted in [`super::blobs::BlobStore`] outside
/// any SQLite transaction, carrying GC-blocking permits until the referencing
/// transaction commits.
#[derive(Debug, Default)]
pub(super) struct StagedUploads {
    items: Vec<StagedUpload>,
    total_bytes: usize,
}

#[derive(Debug)]
struct StagedUpload {
    reference: PromptAttachment,
    size: u64,
    _permit: BlobPermit,
}

impl StagedUploads {
    pub(super) const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Validates filenames, decodes base64 payloads, enforces the per-prompt byte
/// cap on new uploads, and writes each blob to disk outside any SQLite lock.
pub(super) async fn stage(
    workspace: &Workspace,
    attachments: Vec<Attachment>,
) -> Result<StagedUploads, Error> {
    let mut remaining = TOTAL_LIMIT;
    let mut total_bytes = 0usize;
    let mut items = Vec::with_capacity(attachments.len());
    for Attachment { name, mime, data } in attachments {
        validate_name(&name)?;
        let decoded_len = data.len() / 4 * 3
            - data
                .bytes()
                .rev()
                .take(2)
                .take_while(|&ch| ch == b'=')
                .count()
                .min(data.len() / 4 * 3);
        if decoded_len > remaining {
            return Err(too_large());
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| Error::invalid("invalid attachment base64"))?;
        remaining = remaining.checked_sub(bytes.len()).ok_or_else(too_large)?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        let (blob_id, size, permit) = workspace.blobs.put(bytes).await?;
        items.push(StagedUpload {
            reference: PromptAttachment {
                id: blob_id,
                name,
                mime,
            },
            size,
            _permit: permit,
        });
    }
    Ok(StagedUploads { items, total_bytes })
}

/// Records staged uploads and resolves thread-owned stored attachment ids
/// under one unified `TOTAL_LIMIT` byte budget for the prompt.
pub(super) fn commit(
    db: &Conn,
    thread: ThreadId,
    staged: &StagedUploads,
    stored_ids: &[String],
) -> Result<Vec<PromptAttachment>, Error> {
    let now = store::now();
    let mut remaining = TOTAL_LIMIT
        .checked_sub(staged.total_bytes)
        .ok_or_else(too_large)?;
    let mut attachments = Vec::with_capacity(staged.items.len() + stored_ids.len());
    for upload in &staged.items {
        exec(
            db,
            "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT DO NOTHING",
            [
                upload.reference.id.as_str().into(),
                upload.size.into(),
                upload.reference.mime.as_str().into(),
                now.into(),
            ],
        )?;
        exec(
            db,
            "INSERT INTO thread_attachments (thread, id, blob, name, mime)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(thread, id) DO UPDATE SET name = excluded.name, mime = excluded.mime",
            [
                thread.into(),
                upload.reference.id.as_str().into(),
                upload.reference.id.as_str().into(),
                upload.reference.name.as_str().into(),
                upload.reference.mime.as_str().into(),
            ],
        )?;
        attachments.push(upload.reference.clone());
    }
    for id in stored_ids {
        let info = owned(db, thread, id)?;
        let size = usize::try_from(info.size).map_err(|_| too_large())?;
        remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
        attachments.push(PromptAttachment {
            id: info.id,
            name: info.name,
            mime: info.mime,
        });
    }
    Ok(attachments)
}

/// Links peer-pushed content-addressed blobs into `thread_attachments` inside
/// `db`, verifying each blob exists in `blobs`, matches its declared size, and
/// stays within the unified prompt byte budget.
pub(super) fn link_peer(
    db: &Conn,
    thread: ThreadId,
    refs: &[PeerAttachment],
) -> Result<Vec<PromptAttachment>, Error> {
    let mut remaining = TOTAL_LIMIT;
    let mut attachments = Vec::with_capacity(refs.len());
    for item in refs {
        validate_name(&item.name)?;
        if !super::blobs::is_valid_blob_id(&item.blob) {
            return Err(Error::invalid("invalid peer blob identifier"));
        }
        let stored_size = row(
            db,
            "SELECT size FROM blobs WHERE id = ?1",
            [item.blob.as_str().into()],
        )?
        .ok_or_else(|| Error::unavailable(format!("blob {} has not been pushed yet", item.blob)))?
        .u64(0)?;
        if stored_size != item.size {
            return Err(Error::invalid("peer attachment size mismatch"));
        }
        let size = usize::try_from(stored_size).map_err(|_| too_large())?;
        remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
        exec(
            db,
            "INSERT INTO thread_attachments (thread, id, blob, name, mime)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(thread, id) DO UPDATE SET name = excluded.name, mime = excluded.mime",
            [
                thread.into(),
                item.blob.as_str().into(),
                item.blob.as_str().into(),
                item.name.as_str().into(),
                item.mime.as_str().into(),
            ],
        )?;
        attachments.push(PromptAttachment {
            id: item.blob.clone(),
            name: item.name.clone(),
            mime: item.mime.clone(),
        });
    }
    Ok(attachments)
}

/// Validates and stores raw uploaded attachment bytes for `thread`, returning
/// its persisted [`BlobInfo`].
pub(super) async fn store_raw(
    workspace: &Workspace,
    thread: ThreadId,
    name: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<BlobInfo, Error> {
    validate_name(&name)?;
    if bytes.len() > TOTAL_LIMIT {
        return Err(too_large());
    }
    let (blob_id, size, permit) = workspace.blobs.put(bytes).await?;
    let now = store::now();
    let blob_for_tx = blob_id.clone();
    let name_for_tx = name.clone();
    let mime_for_tx = mime.clone();
    workspace
        .db
        .write_tx(move |tx| {
            store::require(tx, thread)?;
            exec(
                tx,
                "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT DO NOTHING",
                [
                    blob_for_tx.as_str().into(),
                    size.into(),
                    mime_for_tx.as_str().into(),
                    now.into(),
                ],
            )?;
            exec(
                tx,
                "INSERT INTO thread_attachments (thread, id, blob, name, mime)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(thread, id) DO UPDATE SET name = excluded.name, mime = excluded.mime",
                [
                    thread.into(),
                    blob_for_tx.as_str().into(),
                    blob_for_tx.as_str().into(),
                    name_for_tx.as_str().into(),
                    mime_for_tx.as_str().into(),
                ],
            )?;
            Ok(())
        })
        .await?;
    drop(permit);
    Ok(BlobInfo {
        id: blob_id.clone(),
        blob: blob_id,
        name,
        mime,
        size,
    })
}

/// Converts a parent thread's newly supplied and stored attachments into
/// content-addressed [`PeerAttachment`] references for durable delegation,
/// enforcing the unified prompt byte budget.
pub(super) async fn prepare_delegated(
    workspace: &Workspace,
    parent: ThreadId,
    uploads: Vec<Attachment>,
    stored_ids: &[String],
) -> Result<Vec<PeerAttachment>, Error> {
    if uploads.is_empty() && stored_ids.is_empty() {
        return Ok(Vec::new());
    }
    let staged = stage(workspace, uploads).await?;
    let stored_ids = stored_ids.to_vec();
    let out = workspace
        .db
        .write_tx(move |tx| {
            store::require(tx, parent)?;
            let refs = commit(tx, parent, &staged, &stored_ids)?;
            let mut out = Vec::with_capacity(refs.len());
            for item in refs {
                let info = owned(tx, parent, &item.id)?;
                out.push(PeerAttachment {
                    blob: info.blob,
                    name: item.name,
                    mime: item.mime,
                    size: info.size,
                });
            }
            drop(staged);
            Ok(out)
        })
        .await?;
    Ok(out)
}

/// Returns thread-scoped attachment metadata without leaking whether a blob
/// exists on any other thread.
pub(super) fn stat(db: &Conn, thread: ThreadId, id: &str) -> Result<BlobInfo, Error> {
    store::require(db, thread)?;
    lookup(db, thread, id)?.ok_or_else(|| {
        Error::not_found(format!(
            "attachment {id} does not belong to thread {thread}"
        ))
    })
}

/// Loads thread-owned attachments into wire [`Attachment`] values, enforcing
/// membership on the WAL reader pool and reading bytes from disk outside it.
pub(super) async fn load_inputs(
    workspace: &Workspace,
    thread: ThreadId,
    ids: &[String],
) -> Result<Vec<Attachment>, Error> {
    let ids = ids.to_vec();
    let entries = workspace
        .db
        .read_snapshot(move |tx| {
            store::require(tx, thread)?;
            let mut remaining = TOTAL_LIMIT;
            let mut found = Vec::with_capacity(ids.len());
            for id in &ids {
                let info = owned(tx, thread, id)?;
                let size = usize::try_from(info.size).map_err(|_| too_large())?;
                remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
                found.push(info);
            }
            Ok(found)
        })
        .await?;
    let mut attachments = Vec::with_capacity(entries.len());
    for info in entries {
        let bytes = workspace
            .blobs
            .read(&info.blob, info.size.min(TOTAL_LIMIT as u64))
            .await?;
        attachments.push(Attachment {
            name: info.name,
            mime: info.mime,
            data: STANDARD.encode(bytes),
        });
    }
    Ok(attachments)
}

/// Materializes isolated read-only copies of a run's attachments on disk for
/// an external ACP harness without holding any SQLite lock during file I/O.
pub(super) async fn materialize(
    workspace: &Workspace,
    thread: ThreadId,
    run: RunId,
    refs: Vec<PromptAttachment>,
) -> Result<Vec<StoredAttachment>, Error> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = workspace
        .db
        .read_snapshot(move |tx| {
            let mut resolved = Vec::with_capacity(refs.len());
            for item in refs {
                let info = owned(tx, thread, &item.id)?;
                resolved.push((item, info.blob, info.size));
            }
            Ok(resolved)
        })
        .await?;
    let parent = workspace.attachment_dir(thread).join(run.to_string());
    let mut written = HashSet::new();
    let mut saved = Vec::with_capacity(resolved.len());
    for (PromptAttachment { id, name, mime }, blob, size) in resolved {
        validate_name(&name)?;
        let directory = parent.join(&id);
        let path = if written.insert((id.clone(), name.clone())) {
            workspace
                .blobs
                .materialize_copy(&blob, &directory, &name, size.min(TOTAL_LIMIT as u64))
                .await?
        } else {
            directory.join(&name)
        };
        saved.push(StoredAttachment {
            id,
            name,
            mime,
            path,
        });
    }
    Ok(saved)
}

fn owned(db: &Conn, thread: ThreadId, id: &str) -> Result<BlobInfo, Error> {
    lookup(db, thread, id)?.ok_or_else(|| {
        Error::invalid(format!(
            "attachment {id} does not belong to thread {thread}"
        ))
    })
}

fn lookup(db: &Conn, thread: ThreadId, id: &str) -> Result<Option<BlobInfo>, Error> {
    let Some(mut row) = row(
        db,
        "SELECT ta.id, ta.blob, ta.name, ta.mime, b.size
         FROM thread_attachments AS ta
         JOIN blobs AS b ON b.id = ta.blob
         WHERE ta.thread = ?1 AND ta.id = ?2",
        [thread.into(), id.into()],
    )?
    else {
        return Ok(None);
    };
    Ok(Some(BlobInfo {
        id: row.str(0)?,
        blob: row.str(1)?,
        name: row.str(2)?,
        mime: row.str(3)?,
        size: row.u64(4)?,
    }))
}

fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name
            .bytes()
            .any(|ch| matches!(ch, b'/' | b'\\' | 0..=31 | 127))
    {
        return Err(Error::invalid("invalid attachment name"));
    }
    Ok(())
}

fn too_large() -> Error {
    Error::invalid("attachments exceed 20 MiB total")
}
