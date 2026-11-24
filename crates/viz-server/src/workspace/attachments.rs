//! Attachment blob staging, principal-scoped claim authorization, thread
//! membership checks, agent media blob extraction, unified prompt byte
//! budgeting, and read-only harness materialization.

use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use viz_protocol::{
    BlobAttachment, BlobInfo, Body, ContentKind, LiveTurn, Media, MediaBlob, RunId, ThreadId,
    peer::PeerAttachment,
};

use super::{
    Workspace, blobs,
    reconcile::{BlobPermit, ResourceLeases},
    sql::{Conn, exec, row},
    store,
};
use crate::{
    acp::Output,
    error::Error,
    wire::{PromptAttachment, StoredAttachment},
};

const TOTAL_LIMIT: usize = viz_protocol::limits::ATTACHMENT_TOTAL_BYTES;

/// Agent-emitted binary media already persisted in [`super::blobs::BlobStore`]
/// outside SQLite, holding GC-blocking permits until its `blobs` and
/// `thread_attachments` rows commit.
#[derive(Debug)]
pub(super) struct ExtractedMedia {
    blob: String,
    mime: String,
    size: u64,
    _permit: BlobPermit,
}

/// Validates and persists raw bytes uploaded via `ContentRequest::UploadStage`,
/// recording a `blobs` row and granting `principal_key` a temporary stage lease
/// so a subsequent `thread.launch`, `thread.send`, `task.delegate`, or
/// `project.update` can claim the blob without re-sending bytes.
pub(super) async fn stage_raw(
    workspace: &Workspace,
    principal_key: &str,
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
    let blob_for_write = blob_id.clone();
    let mime_for_write = mime.clone();
    workspace
        .db
        .write(move |conn| {
            exec(
                conn,
                "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT DO NOTHING",
                [
                    blob_for_write.as_str().into(),
                    size.into(),
                    mime_for_write.as_str().into(),
                    now.into(),
                ],
            )?;
            Ok(())
        })
        .await?;
    workspace.leases.mark_client_staged(principal_key, &blob_id);
    drop(permit);
    Ok(BlobInfo {
        id: blob_id.clone(),
        blob: blob_id,
        name,
        mime,
        size,
    })
}

/// Authorizes and links content-plane staged blobs and thread-owned stored
/// attachment ids into `thread_attachments` under one unified `TOTAL_LIMIT`
/// prompt byte budget.
pub(super) fn claim_staged(
    db: &Conn,
    leases: &ResourceLeases,
    principal_key: &str,
    thread: ThreadId,
    staged: &[BlobAttachment],
    stored_ids: &[String],
) -> Result<Vec<PromptAttachment>, Error> {
    let mut remaining = TOTAL_LIMIT;
    let mut attachments = Vec::with_capacity(staged.len() + stored_ids.len());
    for item in staged {
        validate_name(&item.name)?;
        if !blobs::is_valid_blob_id(&item.blob) {
            return Err(Error::invalid("invalid attachment blob identifier"));
        }
        let authorized = leases.has_client_stage(principal_key, &item.blob)
            || lookup(db, thread, &item.blob)?.is_some();
        if !authorized {
            return Err(Error::invalid(format!(
                "attachment blob {} is not staged by this client or owned by thread {thread}",
                item.blob
            )));
        }
        let stored_size = row(
            db,
            "SELECT size FROM blobs WHERE id = ?1",
            [item.blob.as_str().into()],
        )?
        .ok_or_else(|| Error::invalid(format!("attachment blob {} is not stored", item.blob)))?
        .u64(0)?;
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

/// Extracts inline base64 payloads from agent-emitted [`Body::Media`] blocks
/// into [`super::blobs::BlobStore`], replacing the inline bytes with a
/// [`MediaBlob`] reference before entries are persisted or broadcast.
pub(super) async fn extract_media_blobs(
    workspace: &Workspace,
    entries: &mut [Output],
    live: Option<&mut LiveTurn>,
) -> Result<Vec<ExtractedMedia>, Error> {
    let mut extracted = Vec::new();
    for output in entries {
        if let Output::Entry(entry) = output
            && let Body::Media(media) = &mut entry.body
            && let Some(item) = extract_media(workspace, media).await?
        {
            extracted.push(item);
        }
    }
    if let Some(live) = live {
        for entry in &mut live.entries {
            if let Body::Media(media) = &mut entry.body
                && let Some(item) = extract_media(workspace, media).await?
            {
                extracted.push(item);
            }
        }
    }
    Ok(extracted)
}

async fn extract_media(
    workspace: &Workspace,
    media: &mut Media,
) -> Result<Option<ExtractedMedia>, Error> {
    if media.blob.is_some() {
        return Ok(None);
    }
    let (raw_base64, mime) = match media.block.kind {
        ContentKind::Image | ContentKind::Audio => {
            let Some(obj) = media.block.payload.as_object() else {
                return Ok(None);
            };
            let Some(data) = obj
                .get("data")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                return Ok(None);
            };
            let mime = obj
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_owned();
            (data.to_owned(), mime)
        }
        ContentKind::Resource => {
            let Some(resource) = media
                .block
                .payload
                .get("resource")
                .and_then(Value::as_object)
            else {
                return Ok(None);
            };
            let Some(data) = resource
                .get("blob")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                return Ok(None);
            };
            let mime = resource
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_owned();
            (data.to_owned(), mime)
        }
        ContentKind::Text | ContentKind::ResourceLink | ContentKind::Other => return Ok(None),
    };
    let Ok(bytes) = STANDARD.decode(&raw_base64) else {
        tracing::warn!("skipping invalid base64 payload in agent media block");
        return Ok(None);
    };
    if bytes.len() > TOTAL_LIMIT {
        tracing::warn!(
            size = bytes.len(),
            "dropping oversized inline base64 payload in agent media block"
        );
        strip_inline_media_data(media);
        return Ok(None);
    }
    let (blob_id, size, permit) = workspace.blobs.put(bytes).await?;
    strip_inline_media_data(media);
    media.blob = Some(MediaBlob {
        id: blob_id.clone(),
        mime: mime.clone(),
        size,
    });
    Ok(Some(ExtractedMedia {
        blob: blob_id,
        mime,
        size,
        _permit: permit,
    }))
}

fn strip_inline_media_data(media: &mut Media) {
    match media.block.kind {
        ContentKind::Image | ContentKind::Audio => {
            if let Some(obj) = media.block.payload.as_object_mut() {
                obj.remove("data");
            }
        }
        ContentKind::Resource => {
            if let Some(resource) = media
                .block
                .payload
                .get_mut("resource")
                .and_then(Value::as_object_mut)
            {
                resource.remove("blob");
            }
        }
        ContentKind::Text | ContentKind::ResourceLink | ContentKind::Other => {}
    }
}

/// Commits `blobs` and `thread_attachments` membership rows for agent-emitted
/// media extracted by [`extract_media_blobs`].
pub(super) fn link_media_blobs(
    db: &Conn,
    thread: ThreadId,
    items: &[ExtractedMedia],
) -> Result<(), Error> {
    if items.is_empty() {
        return Ok(());
    }
    let now = store::now();
    for item in items {
        let name = format!("media-{}", &item.blob[..8]);
        exec(
            db,
            "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT DO NOTHING",
            [
                item.blob.as_str().into(),
                item.size.into(),
                item.mime.as_str().into(),
                now.into(),
            ],
        )?;
        exec(
            db,
            "INSERT INTO thread_attachments (thread, id, blob, name, mime)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(thread, id) DO NOTHING",
            [
                thread.into(),
                item.blob.as_str().into(),
                item.blob.as_str().into(),
                name.as_str().into(),
                item.mime.as_str().into(),
            ],
        )?;
    }
    Ok(())
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
        if !blobs::is_valid_blob_id(&item.blob) {
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

/// Converts a parent thread's staged and stored attachments into
/// content-addressed [`PeerAttachment`] references for durable delegation,
/// enforcing the unified prompt byte budget.
pub(super) async fn prepare_delegated(
    workspace: &Workspace,
    principal_key: &str,
    parent: ThreadId,
    uploads: Vec<BlobAttachment>,
    stored_ids: &[String],
) -> Result<Vec<PeerAttachment>, Error> {
    if uploads.is_empty() && stored_ids.is_empty() {
        return Ok(Vec::new());
    }
    let principal_key = principal_key.to_owned();
    let stored_ids = stored_ids.to_vec();
    let leases = std::sync::Arc::clone(&workspace.leases);
    let out = workspace
        .db
        .write_tx(move |tx| {
            store::require(tx, parent)?;
            let refs = claim_staged(tx, &leases, &principal_key, parent, &uploads, &stored_ids)?;
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
