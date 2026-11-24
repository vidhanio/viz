//! Dedicated binary content-plane WebSocket session
//! (`ConnectionChannel::Content`).
//!
//! Handles pull-based 256 KiB chunk reads for attachments, workspace files,
//! and project icons plus disk-streamed binary uploads without contending with
//! the JSON-RPC control socket or buffering 20 MiB uploads in RAM.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinSet,
};
use viz_protocol::{
    ContentRequest, ContentResponse, MethodScope, decode_content_frame_offset,
    encode_content_frame, limits,
};

use super::Server;
use crate::{error::Error, rpc::CallCtx, wire::error_code, workspace::StagedUpload};

const MAX_IN_FLIGHT: usize = 8;
const MAX_CONCURRENT_UPLOADS: usize = 4;
const OUTGOING_CAPACITY: usize = 32;

/// Runs a content-plane WebSocket session after the `hello` handshake.
pub(super) async fn session(socket: WebSocket, server: Arc<Server>, ctx: CallCtx) -> Result<()> {
    let (mut write, mut read) = socket.split();
    let (outgoing, mut messages) = mpsc::channel::<Message>(OUTGOING_CAPACITY);
    let stop = server.stop.clone();
    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                frame = messages.recv() => {
                    let Some(frame) = frame else { break };
                    if write.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let uploads: Arc<Mutex<HashMap<u64, StagedUpload>>> = Arc::default();
    let mut jobs = JoinSet::new();

    let result = async {
        loop {
            tokio::select! {
                () = server.stop.cancelled() => break,
                () = outgoing.closed() => break,
                _ = jobs.join_next(), if !jobs.is_empty() => {},
                frame = read.next() => {
                    let Some(frame) = frame else { break };
                    let frame = frame?;
                    match frame {
                        Message::Close(_) => break,
                        Message::Binary(bytes) => {
                            let (request, payload) = match decode_content_frame_offset::<ContentRequest>(&bytes) {
                                Ok((req, payload_start)) => (req, bytes.slice(payload_start..)),
                                Err(error) => {
                                    send_error(&outgoing, 0, &Error::invalid(error.to_string())).await?;
                                    continue;
                                }
                            };
                            if jobs.len() >= MAX_IN_FLIGHT {
                                send_error(
                                    &outgoing,
                                    request.id(),
                                    &Error::unavailable("too many content requests in flight"),
                                )
                                .await?;
                                continue;
                            }
                            let server = Arc::clone(&server);
                            let outgoing = outgoing.clone();
                            let uploads = Arc::clone(&uploads);
                            let ctx = ctx.clone();
                            jobs.spawn(async move {
                                if let Err(error) =
                                    handle_request(&server, &ctx, &uploads, &outgoing, request, payload).await
                                {
                                    tracing::debug!(%error, "content request ended");
                                }
                            });
                        }
                        Message::Text(_) => {
                            send_error(
                                &outgoing,
                                0,
                                &Error::invalid("content channel accepts binary frames only"),
                            )
                            .await?;
                        }
                        Message::Ping(_) | Message::Pong(_) => {}
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    jobs.abort_all();
    writer.abort();
    result
}

const fn request_scope(request: &ContentRequest) -> (MethodScope, &'static str) {
    let attachment = MethodScope::OPERATOR.union(MethodScope::CONNECTOR);
    match request {
        ContentRequest::GetAttachment { .. } => (attachment, "content.getAttachment"),
        ContentRequest::GetFile { .. } => (MethodScope::OPERATOR, "content.getFile"),
        ContentRequest::GetProjectIcon { .. } => (MethodScope::OPERATOR, "content.getProjectIcon"),
        ContentRequest::GetEntry { .. } => (attachment, "content.getEntry"),
        ContentRequest::UploadChunk { .. } => (attachment, "content.uploadChunk"),
        ContentRequest::UploadStage { .. } => (attachment, "content.uploadStage"),
        ContentRequest::UploadFinish { .. } => (attachment, "content.uploadFinish"),
        ContentRequest::UploadAbort { .. } => (attachment, "content.uploadAbort"),
    }
}

async fn handle_request(
    server: &Server,
    ctx: &CallCtx,
    uploads: &Mutex<HashMap<u64, StagedUpload>>,
    outgoing: &mpsc::Sender<Message>,
    request: ContentRequest,
    payload: Bytes,
) -> Result<()> {
    let (scope, method) = request_scope(&request);
    if let Err(error) = ctx.require(scope, method) {
        return send_error(outgoing, request.id(), &error).await;
    }
    match request {
        ContentRequest::GetAttachment {
            id,
            thread,
            attachment,
            variant,
            offset,
            len,
        } => {
            let reply = server
                .workspace
                .read_attachment_chunk(thread, &attachment, variant, offset, clamp_chunk_len(len))
                .await;
            send_chunk_reply(outgoing, id, offset, reply).await?;
        }
        ContentRequest::GetFile {
            id,
            root,
            path,
            variant,
            offset,
            len,
        } => {
            let reply = server
                .workspace
                .read_file_chunk(root, &path, variant, offset, clamp_chunk_len(len))
                .await;
            send_chunk_reply(outgoing, id, offset, reply).await?;
        }
        ContentRequest::GetProjectIcon {
            id,
            project,
            etag,
            offset,
            len,
        } => {
            let reply = server
                .workspace
                .read_project_icon_chunk(&project, etag.as_deref(), offset, clamp_chunk_len(len))
                .await;
            send_project_icon_reply(outgoing, id, offset, reply).await?;
        }
        ContentRequest::GetEntry {
            id,
            thread,
            entry,
            rev,
            offset,
            len,
        } => {
            let reply = server
                .workspace
                .read_entry_chunk(thread, entry, rev, offset, clamp_chunk_len(len))
                .await;
            send_entry_chunk_reply(outgoing, id, offset, reply).await?;
        }
        ContentRequest::UploadChunk { .. }
        | ContentRequest::UploadStage { .. }
        | ContentRequest::UploadFinish { .. }
        | ContentRequest::UploadAbort { .. } => {
            handle_upload_request(server, ctx, uploads, outgoing, request, payload).await?;
        }
    }
    Ok(())
}

async fn handle_upload_request(
    server: &Server,
    ctx: &CallCtx,
    uploads: &Mutex<HashMap<u64, StagedUpload>>,
    outgoing: &mpsc::Sender<Message>,
    request: ContentRequest,
    payload: Bytes,
) -> Result<()> {
    match request {
        ContentRequest::UploadChunk { id, upload, offset } => {
            match append_upload_chunk(server, uploads, upload, offset, payload).await {
                Ok(received) => send_upload_ack(outgoing, id, upload, received).await?,
                Err(error) => send_error(outgoing, id, &error).await?,
            }
        }
        ContentRequest::UploadStage {
            id,
            upload,
            name,
            mime,
        } => {
            let staged = uploads.lock().await.remove(&upload);
            let reply = server
                .workspace
                .stage_committed_blob(&ctx.principal.key(), name, mime, staged)
                .await;
            send_uploaded_reply(outgoing, id, reply).await?;
        }
        ContentRequest::UploadFinish {
            id,
            upload,
            thread,
            name,
            mime,
        } => {
            let staged = uploads.lock().await.remove(&upload);
            let reply = server
                .workspace
                .store_committed_attachment(thread, name, mime, staged)
                .await;
            send_uploaded_reply(outgoing, id, reply).await?;
        }
        ContentRequest::UploadAbort { id, upload } => {
            uploads.lock().await.remove(&upload);
            send_upload_ack(outgoing, id, upload, 0).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn send_upload_ack(
    outgoing: &mpsc::Sender<Message>,
    id: u64,
    upload: u64,
    received: u64,
) -> Result<()> {
    send_frame(
        outgoing,
        &ContentResponse::UploadAck {
            id,
            upload,
            received,
        },
        &[],
    )
    .await
}

fn clamp_chunk_len(len: u32) -> usize {
    usize::try_from(len)
        .unwrap_or(limits::CONTENT_CHUNK_BYTES)
        .clamp(1, limits::CONTENT_CHUNK_BYTES)
}

async fn send_uploaded_reply(
    outgoing: &mpsc::Sender<Message>,
    id: u64,
    reply: Result<viz_protocol::BlobInfo, Error>,
) -> Result<()> {
    match reply {
        Ok(info) => send_frame(outgoing, &ContentResponse::Uploaded { id, info }, &[]).await,
        Err(error) => send_error(outgoing, id, &error).await,
    }
}

async fn send_project_icon_reply(
    outgoing: &mpsc::Sender<Message>,
    id: u64,
    offset: u64,
    reply: Result<crate::workspace::ProjectIconChunk, Error>,
) -> Result<()> {
    match reply {
        Ok(crate::workspace::ProjectIconChunk::Chunk {
            bytes,
            total_size,
            mime,
            etag,
        }) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= total_size;
            send_frame(
                outgoing,
                &ContentResponse::Chunk {
                    id,
                    total_size,
                    mime,
                    etag: Some(etag),
                    rev: None,
                    offset,
                    eof,
                },
                &bytes,
            )
            .await
        }
        Ok(crate::workspace::ProjectIconChunk::NotModified { etag }) => {
            send_frame(outgoing, &ContentResponse::NotModified { id, etag }, &[]).await
        }
        Ok(crate::workspace::ProjectIconChunk::Empty) => {
            send_frame(outgoing, &ContentResponse::Empty { id }, &[]).await
        }
        Err(error) => send_error(outgoing, id, &error).await,
    }
}

async fn send_entry_chunk_reply(
    outgoing: &mpsc::Sender<Message>,
    id: u64,
    offset: u64,
    reply: Result<(Vec<u8>, u64, u64), Error>,
) -> Result<()> {
    match reply {
        Ok((chunk, total_size, rev)) => {
            let eof = offset.saturating_add(chunk.len() as u64) >= total_size;
            send_frame(
                outgoing,
                &ContentResponse::Chunk {
                    id,
                    total_size,
                    mime: "application/json".to_owned(),
                    etag: None,
                    rev: Some(rev),
                    offset,
                    eof,
                },
                &chunk,
            )
            .await
        }
        Err(error) => send_error(outgoing, id, &error).await,
    }
}

async fn send_chunk_reply(
    outgoing: &mpsc::Sender<Message>,
    id: u64,
    offset: u64,
    reply: Result<(Vec<u8>, u64, String), Error>,
) -> Result<()> {
    match reply {
        Ok((chunk, total_size, mime)) => {
            let eof = offset.saturating_add(chunk.len() as u64) >= total_size;
            send_frame(
                outgoing,
                &ContentResponse::Chunk {
                    id,
                    total_size,
                    mime,
                    etag: None,
                    rev: None,
                    offset,
                    eof,
                },
                &chunk,
            )
            .await
        }
        Err(error) => send_error(outgoing, id, &error).await,
    }
}

async fn append_upload_chunk(
    server: &Server,
    uploads: &Mutex<HashMap<u64, StagedUpload>>,
    upload: u64,
    offset: u64,
    payload: Bytes,
) -> Result<u64, Error> {
    let existing = {
        let map = uploads.lock().await;
        if let Some(handle) = map.get(&upload) {
            Some(handle.clone())
        } else if map.len() >= MAX_CONCURRENT_UPLOADS {
            drop(map);
            return Err(Error::unavailable("too many concurrent uploads"));
        } else {
            None
        }
    };
    let handle = if let Some(handle) = existing {
        handle
    } else {
        let created = server.workspace.begin_upload().await?;
        let mut map = uploads.lock().await;
        if let Some(existing) = map.get(&upload) {
            existing.clone()
        } else if map.len() >= MAX_CONCURRENT_UPLOADS {
            drop(map);
            return Err(Error::unavailable("too many concurrent uploads"));
        } else {
            map.insert(upload, created.clone());
            created
        }
    };
    let result = handle.append(offset, payload).await;
    if result.is_err() {
        uploads.lock().await.remove(&upload);
    }
    result
}

async fn send_error(outgoing: &mpsc::Sender<Message>, id: u64, error: &Error) -> Result<()> {
    send_frame(
        outgoing,
        &ContentResponse::Error {
            id,
            code: error_code(error),
            message: error.to_string(),
        },
        &[],
    )
    .await
}

async fn send_frame(
    outgoing: &mpsc::Sender<Message>,
    header: &ContentResponse,
    payload: &[u8],
) -> Result<()> {
    let bytes = encode_content_frame(header, payload)?;
    let _ = outgoing.send(Message::Binary(bytes.into())).await;
    Ok(())
}
