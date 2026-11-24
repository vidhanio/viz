//! Dedicated binary content-plane WebSocket session
//! (`ConnectionChannel::Content`).
//!
//! Handles pull-based 256 KiB chunk reads for attachments and workspace files
//! (original or server-rendered thumbnails) plus streamed binary attachment
//! uploads without contending with the JSON-RPC control socket.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinSet,
};
use viz_protocol::{
    ContentRequest, ContentResponse, MethodScope, decode_content_frame, encode_content_frame,
    limits,
};

use super::Server;
use crate::{error::Error, rpc::CallCtx, wire::error_code};

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

    let uploads: Arc<Mutex<HashMap<u64, Vec<u8>>>> = Arc::default();
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
                            let (request, payload) = match decode_content_frame::<ContentRequest>(&bytes) {
                                Ok((req, payload)) => (req, payload.to_vec()),
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

async fn handle_request(
    server: &Server,
    ctx: &CallCtx,
    uploads: &Mutex<HashMap<u64, Vec<u8>>>,
    outgoing: &mpsc::Sender<Message>,
    request: ContentRequest,
    payload: Vec<u8>,
) -> Result<()> {
    let attachment_scope = MethodScope::OPERATOR.union(MethodScope::CONNECTOR);
    match request {
        ContentRequest::GetAttachment {
            id,
            thread,
            attachment,
            variant,
            offset,
            len,
        } => {
            if let Err(error) = ctx.require(attachment_scope, "content.getAttachment") {
                return send_error(outgoing, id, &error).await;
            }
            let capped = clamp_chunk_len(len);
            let reply = server
                .workspace
                .read_attachment_chunk(thread, &attachment, variant, offset, capped)
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
            if let Err(error) = ctx.require(MethodScope::OPERATOR, "content.getFile") {
                return send_error(outgoing, id, &error).await;
            }
            let capped = clamp_chunk_len(len);
            let reply = server
                .workspace
                .read_file_chunk(root, &path, variant, offset, capped)
                .await;
            send_chunk_reply(outgoing, id, offset, reply).await?;
        }
        ContentRequest::UploadChunk { id, upload, offset } => {
            if let Err(error) = ctx.require(attachment_scope, "content.uploadChunk") {
                return send_error(outgoing, id, &error).await;
            }
            match append_upload_chunk(uploads, upload, offset, &payload).await {
                Ok(received) => {
                    send_frame(
                        outgoing,
                        &ContentResponse::UploadAck {
                            id,
                            upload,
                            received,
                        },
                        &[],
                    )
                    .await?;
                }
                Err(error) => send_error(outgoing, id, &error).await?,
            }
        }
        ContentRequest::UploadFinish {
            id,
            upload,
            thread,
            name,
            mime,
        } => {
            if let Err(error) = ctx.require(attachment_scope, "content.uploadFinish") {
                return send_error(outgoing, id, &error).await;
            }
            let bytes = { uploads.lock().await.remove(&upload) }.unwrap_or_default();
            match server
                .workspace
                .store_uploaded_attachment(thread, name, mime, bytes)
                .await
            {
                Ok(info) => {
                    send_frame(outgoing, &ContentResponse::Uploaded { id, info }, &[]).await?;
                }
                Err(error) => send_error(outgoing, id, &error).await?,
            }
        }
        ContentRequest::UploadAbort { id, upload } => {
            if let Err(error) = ctx.require(attachment_scope, "content.uploadAbort") {
                return send_error(outgoing, id, &error).await;
            }
            uploads.lock().await.remove(&upload);
            send_frame(
                outgoing,
                &ContentResponse::UploadAck {
                    id,
                    upload,
                    received: 0,
                },
                &[],
            )
            .await?;
        }
    }
    Ok(())
}

fn clamp_chunk_len(len: u32) -> usize {
    usize::try_from(len)
        .unwrap_or(limits::CONTENT_CHUNK_BYTES)
        .clamp(1, limits::CONTENT_CHUNK_BYTES)
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
    uploads: &Mutex<HashMap<u64, Vec<u8>>>,
    upload: u64,
    offset: u64,
    payload: &[u8],
) -> Result<u64, Error> {
    let mut map = uploads.lock().await;
    if !map.contains_key(&upload) && map.len() >= MAX_CONCURRENT_UPLOADS {
        drop(map);
        return Err(Error::unavailable("too many concurrent uploads"));
    }
    let buf = map.entry(upload).or_default();
    if offset != buf.len() as u64 {
        drop(map);
        return Err(Error::invalid("out-of-order upload chunk offset"));
    }
    if buf.len().saturating_add(payload.len()) > limits::ATTACHMENT_TOTAL_BYTES {
        map.remove(&upload);
        drop(map);
        return Err(Error::invalid("attachment exceeds 20 MiB limit"));
    }
    buf.extend_from_slice(payload);
    let received = buf.len() as u64;
    drop(map);
    Ok(received)
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
