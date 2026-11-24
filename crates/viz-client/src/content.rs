//! Dedicated content-plane WebSocket client carrying pull-based binary chunks
//! and thumbnails separately from the control-plane RPC connection.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt, stream::FuturesOrdered};
use serde::Deserialize;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message, client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig,
    },
};
use viz_protocol::{
    BlobInfo, ClientInfo, ConnectionChannel, ContentRequest, ContentResponse, ContentVariant,
    EntryId, FileRoot, Hello, HelloParams, Method, PROTOCOL_VERSION, ThreadId,
    decode_content_frame_offset, encode_content_frame, limits,
};

use crate::{Endpoint, Error, Result};

const RPC_PATH: &str = "/v2/rpc";
const OUTGOING_CAPACITY: usize = 64;
const PULL_PIPELINE: usize = 4;

/// A downloaded attachment or workspace file fetched over the content plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedContent {
    /// Decoded raw bytes for the requested variant.
    pub bytes: Vec<u8>,
    /// MIME media type reported by the server (e.g., `image/png` for raster
    /// thumbnails).
    pub mime: String,
    /// Total byte size of the variant.
    pub total_size: u64,
}

/// Outcome of fetching a project's image icon over the content plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectIconFetch {
    /// The project has no raster or SVG image icon.
    None,
    /// The cached `ETag` still matches the current icon.
    Unchanged { etag: String },
    /// Raw icon bytes and metadata.
    Image {
        etag: String,
        mime: String,
        bytes: Vec<u8>,
    },
}

/// Dedicated bulk-content WebSocket handle. Cheap to clone; pull requests may
/// run concurrently without blocking control-plane RPCs.
#[derive(Debug, Clone)]
pub struct ContentClient {
    outgoing: mpsc::Sender<Message>,
    pending: Arc<PendingContent>,
    next_id: Arc<AtomicU64>,
    next_upload: Arc<AtomicU64>,
}

impl ContentClient {
    /// Connects a dedicated content-plane WebSocket to `endpoint`, performing
    /// the standard `hello` handshake with [`ConnectionChannel::Content`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Connect`], [`Error::Rpc`], or [`Error::Protocol`] if
    /// connecting or handshaking fails.
    pub async fn connect(
        endpoint: &Endpoint,
        info: ClientInfo,
        environment: Option<String>,
    ) -> Result<Self> {
        let connect_err = |source: Box<dyn std::error::Error + Send + Sync>| Error::Connect {
            endpoint: endpoint.to_string(),
            source,
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(limits::MAX_CONTENT_FRAME_BYTES))
            .max_frame_size(Some(limits::MAX_CONTENT_FRAME_BYTES));
        let hello_params = HelloParams {
            protocol: PROTOCOL_VERSION,
            client: info,
            environment,
            channel: ConnectionChannel::Content,
        };
        match endpoint {
            Endpoint::Unix { path } => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                let request = format!("ws://localhost{RPC_PATH}")
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let (ws, _) =
                    tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config))
                        .await
                        .map_err(|e| connect_err(e.into()))?;
                Self::finish_connect(ws, hello_params).await
            }
            Endpoint::Http { url, token } => {
                let ws_url = format!("ws{}{RPC_PATH}", &url[4..]);
                let mut request = ws_url
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let auth = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| connect_err(e.into()))?;
                request.headers_mut().insert("authorization", auth);
                let (ws, _) =
                    tokio_tungstenite::connect_async_with_config(request, Some(ws_config), false)
                        .await
                        .map_err(|e| connect_err(e.into()))?;
                Self::finish_connect(ws, hello_params).await
            }
        }
    }

    async fn finish_connect<S>(
        mut ws: WebSocketStream<S>,
        hello_params: HelloParams,
    ) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        #[derive(serde::Serialize)]
        struct HelloRequest<'a> {
            jsonrpc: &'static str,
            id: u64,
            method: &'static str,
            params: &'a HelloParams,
        }
        #[derive(Deserialize)]
        struct HelloResponse {
            result: Option<Hello>,
            error: Option<HelloError>,
        }
        #[derive(Deserialize)]
        struct HelloError {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            message: String,
        }

        let hello_json = serde_json::to_string(&HelloRequest {
            jsonrpc: "2.0",
            id: 1,
            method: HelloParams::NAME,
            params: &hello_params,
        })
        .map_err(|source| Error::Encode {
            method: HelloParams::NAME,
            source,
        })?;
        ws.send(Message::text(hello_json))
            .await
            .map_err(|_| Error::Closed)?;
        let reply_text = loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => break text,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return Err(Error::Closed),
            }
        };
        let reply: HelloResponse =
            serde_json::from_str(reply_text.as_str()).map_err(|source| Error::Decode {
                method: HelloParams::NAME,
                source,
            })?;
        if let Some(error) = reply.error {
            return Err(Error::Rpc {
                method: HelloParams::NAME,
                code: error.code,
                message: error.message,
                git_failure: None,
            });
        }
        let hello = reply.result.ok_or_else(|| Error::Decode {
            method: HelloParams::NAME,
            source: serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "hello response missing result",
            )),
        })?;
        if hello.protocol != PROTOCOL_VERSION {
            return Err(Error::Protocol {
                server: hello.protocol,
            });
        }
        let pending = Arc::<PendingContent>::default();
        let outgoing = spawn_content_io(ws, Arc::clone(&pending));
        Ok(Self {
            outgoing,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            next_upload: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Closes this content WebSocket and fails any in-flight transfers.
    pub fn close(&self) {
        if self.outgoing.try_send(Message::Close(None)).is_err() {
            let outgoing = self.outgoing.clone();
            tokio::spawn(async move {
                let _ = outgoing.send(Message::Close(None)).await;
            });
        }
    }

    /// Pulls a thread-owned attachment (or its server-rendered preview
    /// thumbnail) in 256 KiB binary chunks up to `max_bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn get_attachment(
        &self,
        thread: ThreadId,
        attachment: &str,
        variant: ContentVariant,
        max_bytes: usize,
    ) -> Result<DownloadedContent> {
        self.pull_stream(max_bytes, |id, offset, len| ContentRequest::GetAttachment {
            id,
            thread,
            attachment: attachment.to_owned(),
            variant,
            offset,
            len,
        })
        .await
    }

    /// Pulls a workspace-confined file (or its server-rendered preview
    /// thumbnail) in 256 KiB binary chunks up to `max_bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn get_file(
        &self,
        root: FileRoot,
        path: &str,
        variant: ContentVariant,
        max_bytes: usize,
    ) -> Result<DownloadedContent> {
        self.pull_stream(max_bytes, |id, offset, len| ContentRequest::GetFile {
            id,
            root: root.clone(),
            path: path.to_owned(),
            variant,
            offset,
            len,
        })
        .await
    }

    /// Pulls a stored transcript entry's complete raw JSON bytes over the
    /// content plane in pipelined 256 KiB binary chunks, restarting cleanly if
    /// the entry's revision changes mid-transfer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn get_entry(
        &self,
        thread: ThreadId,
        entry: EntryId,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        const MAX_RETRIES: usize = 3;
        let chunk_len = u32::try_from(limits::CONTENT_CHUNK_BYTES).unwrap_or(u32::MAX);
        for attempt in 0..MAX_RETRIES {
            let first_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let first_req = ContentRequest::GetEntry {
                id: first_id,
                thread,
                entry,
                rev: None,
                offset: 0,
                len: chunk_len,
            };
            let (resp, payload) = self.call(first_id, &first_req, &[]).await?;
            let (total_size, rev, eof) = match resp {
                ContentResponse::Chunk {
                    total_size,
                    offset,
                    eof,
                    rev,
                    ..
                } => {
                    if offset != 0 {
                        return Err(Error::Rpc {
                            method: "content.get_entry",
                            code: -32603,
                            message: "out-of-order entry chunk offset".into(),
                            git_failure: None,
                        });
                    }
                    if total_size > max_bytes as u64 || payload.len() > max_bytes {
                        return Err(Error::Rpc {
                            method: "content.get_entry",
                            code: -32602,
                            message: format!("entry exceeds {max_bytes}-byte client limit"),
                            git_failure: None,
                        });
                    }
                    (total_size, rev, eof)
                }
                ContentResponse::Error { code, message, .. } => {
                    return Err(Error::Rpc {
                        method: "content.get_entry",
                        code: i64::from(code),
                        message,
                        git_failure: None,
                    });
                }
                _ => {
                    return Err(Error::Rpc {
                        method: "content.get_entry",
                        code: -32603,
                        message: "unexpected entry chunk response".into(),
                        git_failure: None,
                    });
                }
            };
            let reserve = usize::try_from(total_size).unwrap_or(0).min(max_bytes);
            let mut bytes = Vec::with_capacity(reserve);
            bytes.extend_from_slice(&payload);
            if eof || payload.is_empty() || (bytes.len() as u64) >= total_size {
                return Ok(bytes);
            }
            match self
                .pull_remaining(
                    "content.get_entry",
                    total_size,
                    max_bytes,
                    &mut bytes,
                    |id, offset, len| ContentRequest::GetEntry {
                        id,
                        thread,
                        entry,
                        rev,
                        offset,
                        len,
                    },
                )
                .await
            {
                Ok(()) => return Ok(bytes),
                Err(Error::Rpc { code: -32002, .. }) if attempt + 1 < MAX_RETRIES => {}
                Err(error) => return Err(error),
            }
        }
        Err(Error::Rpc {
            method: "content.get_entry",
            code: -32002,
            message: "entry changed repeatedly while streaming".into(),
            git_failure: None,
        })
    }

    /// Streams raw bytes in 256 KiB binary chunks and commits them into the
    /// blob store under a principal-scoped staging lease so a subsequent
    /// `thread.launch`, `thread.send`, `task.delegate`, or `project.update`
    /// can claim them without base64 on the control plane.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn stage_blob(&self, name: &str, mime: &str, bytes: &[u8]) -> Result<BlobInfo> {
        let upload = self.stream_upload_chunks(bytes).await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let stage = ContentRequest::UploadStage {
            id,
            upload,
            name: name.to_owned(),
            mime: mime.to_owned(),
        };
        self.finish_upload(id, &stage, "content.upload_stage").await
    }

    /// Streams raw attachment bytes in 256 KiB binary chunks and commits them
    /// directly to `thread`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn upload_attachment(
        &self,
        thread: ThreadId,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<BlobInfo> {
        let upload = self.stream_upload_chunks(bytes).await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let finish = ContentRequest::UploadFinish {
            id,
            upload,
            thread,
            name: name.to_owned(),
            mime: mime.to_owned(),
        };
        self.finish_upload(id, &finish, "content.upload_finish")
            .await
    }

    /// Pulls a project's image icon over the content plane, honoring `etag` to
    /// skip unchanged payloads and pipelining remaining chunks when
    /// multi-chunk.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] on server rejection or [`Error::Closed`] if the
    /// content socket disconnects.
    pub async fn get_project_icon(
        &self,
        project: &str,
        etag: Option<&str>,
        max_bytes: usize,
    ) -> Result<ProjectIconFetch> {
        let chunk_len = u32::try_from(limits::CONTENT_CHUNK_BYTES).unwrap_or(u32::MAX);
        let first_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let first_req = ContentRequest::GetProjectIcon {
            id: first_id,
            project: project.to_owned(),
            etag: etag.map(str::to_owned),
            offset: 0,
            len: chunk_len,
        };
        let (resp, payload) = self.call(first_id, &first_req, &[]).await?;
        let (total_size, mime, resolved_etag, eof) = match resp {
            ContentResponse::Empty { .. } => return Ok(ProjectIconFetch::None),
            ContentResponse::NotModified { etag, .. } => {
                return Ok(ProjectIconFetch::Unchanged { etag });
            }
            ContentResponse::Chunk {
                total_size,
                mime,
                offset,
                eof,
                etag,
                ..
            } => {
                if offset != 0 {
                    return Err(Error::Rpc {
                        method: "content.get_project_icon",
                        code: -32603,
                        message: "out-of-order icon chunk offset".into(),
                        git_failure: None,
                    });
                }
                if total_size > max_bytes as u64 || payload.len() > max_bytes {
                    return Err(Error::Rpc {
                        method: "content.get_project_icon",
                        code: -32602,
                        message: format!("icon exceeds {max_bytes}-byte client limit"),
                        git_failure: None,
                    });
                }
                (total_size, mime, etag.unwrap_or_default(), eof)
            }
            ContentResponse::Error { code, message, .. } => {
                return Err(Error::Rpc {
                    method: "content.get_project_icon",
                    code: i64::from(code),
                    message,
                    git_failure: None,
                });
            }
            _ => {
                return Err(Error::Rpc {
                    method: "content.get_project_icon",
                    code: -32603,
                    message: "unexpected project icon response".into(),
                    git_failure: None,
                });
            }
        };
        let reserve = usize::try_from(total_size).unwrap_or(0).min(max_bytes);
        let mut bytes = Vec::with_capacity(reserve);
        bytes.extend_from_slice(&payload);
        if !eof && !payload.is_empty() && (bytes.len() as u64) < total_size {
            self.pull_remaining(
                "content.get_project_icon",
                total_size,
                max_bytes,
                &mut bytes,
                |id, offset, len| ContentRequest::GetProjectIcon {
                    id,
                    project: project.to_owned(),
                    etag: None,
                    offset,
                    len,
                },
            )
            .await?;
        }
        Ok(ProjectIconFetch::Image {
            etag: resolved_etag,
            mime,
            bytes,
        })
    }

    async fn stream_upload_chunks(&self, bytes: &[u8]) -> Result<u64> {
        let upload = self.next_upload.fetch_add(1, Ordering::Relaxed);
        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = (offset + limits::CONTENT_CHUNK_BYTES).min(bytes.len());
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let req = ContentRequest::UploadChunk {
                id,
                upload,
                offset: offset as u64,
            };
            let (resp, _) = self.call(id, &req, &bytes[offset..end]).await?;
            match resp {
                ContentResponse::UploadAck { .. } => offset = end,
                ContentResponse::Error { code, message, .. } => {
                    return Err(Error::Rpc {
                        method: "content.upload",
                        code: i64::from(code),
                        message,
                        git_failure: None,
                    });
                }
                _ => {
                    return Err(Error::Rpc {
                        method: "content.upload",
                        code: -32603,
                        message: "unexpected content upload response".into(),
                        git_failure: None,
                    });
                }
            }
        }
        Ok(upload)
    }

    async fn finish_upload(
        &self,
        id: u64,
        req: &ContentRequest,
        method: &'static str,
    ) -> Result<BlobInfo> {
        let (resp, _) = self.call(id, req, &[]).await?;
        match resp {
            ContentResponse::Uploaded { info, .. } => Ok(info),
            ContentResponse::Error { code, message, .. } => Err(Error::Rpc {
                method,
                code: i64::from(code),
                message,
                git_failure: None,
            }),
            _ => Err(Error::Rpc {
                method,
                code: -32603,
                message: "unexpected content finish response".into(),
                git_failure: None,
            }),
        }
    }

    async fn pull_stream(
        &self,
        max_bytes: usize,
        mut build: impl FnMut(u64, u64, u32) -> ContentRequest,
    ) -> Result<DownloadedContent> {
        let chunk_len = u32::try_from(limits::CONTENT_CHUNK_BYTES).unwrap_or(u32::MAX);
        let first_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let first_req = build(first_id, 0, chunk_len);
        let (resp, payload) = self.call(first_id, &first_req, &[]).await?;
        let (total_size, mime, eof) = match resp {
            ContentResponse::Chunk {
                total_size,
                mime,
                offset,
                eof,
                ..
            } => {
                if offset != 0 {
                    return Err(Error::Rpc {
                        method: "content.get",
                        code: -32603,
                        message: "out-of-order content chunk offset".into(),
                        git_failure: None,
                    });
                }
                if total_size > max_bytes as u64 || payload.len() > max_bytes {
                    return Err(Error::Rpc {
                        method: "content.get",
                        code: -32602,
                        message: format!("content exceeds {max_bytes}-byte client limit"),
                        git_failure: None,
                    });
                }
                (total_size, mime, eof)
            }
            ContentResponse::Error { code, message, .. } => {
                return Err(Error::Rpc {
                    method: "content.get",
                    code: i64::from(code),
                    message,
                    git_failure: None,
                });
            }
            _ => {
                return Err(Error::Rpc {
                    method: "content.get",
                    code: -32603,
                    message: "unexpected content response".into(),
                    git_failure: None,
                });
            }
        };
        let reserve = usize::try_from(total_size).unwrap_or(0).min(max_bytes);
        let mut bytes = Vec::with_capacity(reserve);
        bytes.extend_from_slice(&payload);
        if !eof && !payload.is_empty() && (bytes.len() as u64) < total_size {
            self.pull_remaining("content.get", total_size, max_bytes, &mut bytes, build)
                .await?;
        }
        Ok(DownloadedContent {
            bytes,
            mime,
            total_size,
        })
    }

    async fn pull_remaining(
        &self,
        method: &'static str,
        total_size: u64,
        max_bytes: usize,
        bytes: &mut Vec<u8>,
        mut build: impl FnMut(u64, u64, u32) -> ContentRequest,
    ) -> Result<()> {
        let chunk_len = u32::try_from(limits::CONTENT_CHUNK_BYTES).unwrap_or(u32::MAX);
        let chunk_step = u64::from(chunk_len);
        let mut next_offset = bytes.len() as u64;
        let mut in_flight = FuturesOrdered::new();
        while in_flight.len() < PULL_PIPELINE && next_offset < total_size {
            let expected = next_offset;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let req = build(id, expected, chunk_len);
            next_offset = next_offset.saturating_add(chunk_step);
            in_flight.push_back(self.fetch_chunk(expected, id, req));
        }
        while let Some((expected, result)) = in_flight.next().await {
            let (resp, payload) = result?;
            match resp {
                ContentResponse::Chunk {
                    offset: reported_offset,
                    eof,
                    ..
                } => {
                    if reported_offset != expected || expected != bytes.len() as u64 {
                        return Err(Error::Rpc {
                            method,
                            code: -32603,
                            message: "out-of-order content chunk offset".into(),
                            git_failure: None,
                        });
                    }
                    if bytes.len().saturating_add(payload.len()) > max_bytes {
                        return Err(Error::Rpc {
                            method,
                            code: -32602,
                            message: format!("content exceeds {max_bytes}-byte client limit"),
                            git_failure: None,
                        });
                    }
                    bytes.extend_from_slice(&payload);
                    if eof || payload.is_empty() {
                        break;
                    }
                    if next_offset < total_size {
                        let expected = next_offset;
                        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                        let req = build(id, expected, chunk_len);
                        next_offset = next_offset.saturating_add(chunk_step);
                        in_flight.push_back(self.fetch_chunk(expected, id, req));
                    }
                }
                ContentResponse::Error { code, message, .. } => {
                    return Err(Error::Rpc {
                        method,
                        code: i64::from(code),
                        message,
                        git_failure: None,
                    });
                }
                _ => {
                    return Err(Error::Rpc {
                        method,
                        code: -32603,
                        message: "unexpected content response".into(),
                        git_failure: None,
                    });
                }
            }
        }
        Ok(())
    }

    async fn fetch_chunk(
        &self,
        expected: u64,
        id: u64,
        req: ContentRequest,
    ) -> (u64, Result<(ContentResponse, Bytes)>) {
        (expected, self.call(id, &req, &[]).await)
    }

    async fn call(
        &self,
        id: u64,
        header: &ContentRequest,
        payload: &[u8],
    ) -> Result<(ContentResponse, Bytes)> {
        let frame = encode_content_frame(header, payload).map_err(|error| Error::Rpc {
            method: "content",
            code: -32602,
            message: error.to_string(),
            git_failure: None,
        })?;
        let (tx, rx) = oneshot::channel();
        let _guard = ContentGuard::insert(&self.pending, id, tx);
        if self
            .outgoing
            .send(Message::Binary(Bytes::from(frame)))
            .await
            .is_err()
        {
            return Err(Error::Closed);
        }
        rx.await.map_err(|_| Error::Closed)
    }
}

type ContentResponder = oneshot::Sender<(ContentResponse, Bytes)>;

#[derive(Default)]
struct PendingContent(Mutex<HashMap<u64, ContentResponder>>);

impl std::fmt::Debug for PendingContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingContent")
            .field("len", &self.0.lock().map_or(0, |m| m.len()))
            .finish()
    }
}

impl PendingContent {
    fn insert(&self, id: u64, tx: ContentResponder) {
        self.0
            .lock()
            .expect("pending-content lock poisoned")
            .insert(id, tx);
    }

    fn take(&self, id: u64) -> Option<ContentResponder> {
        self.0
            .lock()
            .expect("pending-content lock poisoned")
            .remove(&id)
    }

    fn close(&self) {
        self.0
            .lock()
            .expect("pending-content lock poisoned")
            .clear();
    }
}

struct ContentGuard<'a> {
    pending: &'a PendingContent,
    id: u64,
}

impl<'a> ContentGuard<'a> {
    fn insert(pending: &'a PendingContent, id: u64, tx: ContentResponder) -> Self {
        pending.insert(id, tx);
        Self { pending, id }
    }
}

impl Drop for ContentGuard<'_> {
    fn drop(&mut self) {
        let _ = self.pending.take(self.id);
    }
}

fn spawn_content_io<S>(
    ws: WebSocketStream<S>,
    pending: Arc<PendingContent>,
) -> mpsc::Sender<Message>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Message>(OUTGOING_CAPACITY);
    let (mut sink, mut stream) = ws.split();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    let Some(message) = outgoing else { break };
                    let closing = matches!(message, Message::Close(_));
                    if sink.send(message).await.is_err() || closing {
                        break;
                    }
                }
                incoming = stream.next() => {
                    let Some(Ok(message)) = incoming else { break };
                    match message {
                        Message::Binary(bytes) => {
                            match decode_content_frame_offset::<ContentResponse>(&bytes) {
                                Ok((resp, payload_start)) => {
                                    if let Some(responder) = pending.take(resp.id()) {
                                        let _ = responder
                                            .send((resp, bytes.slice(payload_start..)));
                                    }
                                }
                                Err(error) => {
                                    tracing::warn!(%error, "invalid binary content frame from server");
                                    break;
                                }
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        }
        drop(rx);
        pending.close();
    });
    tx
}
