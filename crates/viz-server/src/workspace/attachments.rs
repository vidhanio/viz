//! Attachment decoding, staging into [`super::blobs::BlobStore`], thread
//! membership checks, unified prompt byte budgeting, and read-only harness
//! materialization.

use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use viz_protocol::{BlobInfo, RunId, ThreadId};

use super::{
    Workspace,
    blobs::WritePermit,
    sql::{Connection, exec, row},
    store,
};
use crate::{
    error::Error,
    wire::{Attachment, PromptAttachment, StoredAttachment},
};

const TOTAL_LIMIT: usize = viz_protocol::limits::ATTACHMENT_TOTAL_BYTES;

/// Attachment bytes already persisted in [`super::blobs::BlobStore`] outside
/// the SQLite lock, carrying GC-blocking permits until the referencing
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
    _permit: WritePermit,
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
pub(super) async fn commit(
    db: &mut Connection,
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
        )
        .await?;
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
        )
        .await?;
        attachments.push(upload.reference.clone());
    }
    for id in stored_ids {
        let info = owned(db, thread, id).await?;
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

/// Returns thread-scoped attachment metadata without leaking whether a blob
/// exists on any other thread.
pub(super) async fn stat(
    db: &mut Connection,
    thread: ThreadId,
    id: &str,
) -> Result<BlobInfo, Error> {
    store::require(db, thread).await?;
    lookup(db, thread, id).await?.ok_or_else(|| {
        Error::not_found(format!(
            "attachment {id} does not belong to thread {thread}"
        ))
    })
}

/// Loads thread-owned attachments into wire [`Attachment`] values, enforcing
/// membership under a brief SQLite lock and reading bytes from disk outside it.
pub(super) async fn load_inputs(
    workspace: &Workspace,
    thread: ThreadId,
    ids: &[String],
) -> Result<Vec<Attachment>, Error> {
    let entries = {
        let mut db = workspace.db.lock().await;
        store::require(&mut db, thread).await?;
        let mut remaining = TOTAL_LIMIT;
        let mut found = Vec::with_capacity(ids.len());
        for id in ids {
            let info = owned(&mut db, thread, id).await?;
            let size = usize::try_from(info.size).map_err(|_| too_large())?;
            remaining = remaining.checked_sub(size).ok_or_else(too_large)?;
            found.push(info);
        }
        drop(db);
        found
    };
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
/// an external ACP harness without holding the SQLite lock during file I/O.
pub(super) async fn materialize(
    workspace: &Workspace,
    thread: ThreadId,
    run: RunId,
    refs: Vec<PromptAttachment>,
) -> Result<Vec<StoredAttachment>, Error> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let resolved = {
        let mut db = workspace.db.lock().await;
        let mut resolved = Vec::with_capacity(refs.len());
        for item in refs {
            let info = owned(&mut db, thread, &item.id).await?;
            resolved.push((item, info.blob, info.size));
        }
        drop(db);
        resolved
    };
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

async fn owned(db: &mut Connection, thread: ThreadId, id: &str) -> Result<BlobInfo, Error> {
    lookup(db, thread, id).await?.ok_or_else(|| {
        Error::invalid(format!(
            "attachment {id} does not belong to thread {thread}"
        ))
    })
}

async fn lookup(
    db: &mut Connection,
    thread: ThreadId,
    id: &str,
) -> Result<Option<BlobInfo>, Error> {
    let Some(mut row) = row(
        db,
        "SELECT ta.id, ta.blob, ta.name, ta.mime, b.size
         FROM thread_attachments AS ta
         JOIN blobs AS b ON b.id = ta.blob
         WHERE ta.thread = ?1 AND ta.id = ?2",
        [thread.into(), id.into()],
    )
    .await?
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
