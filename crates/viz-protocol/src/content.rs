//! Dedicated binary content-plane WebSocket framing (`[u32
//! header_len][JSON][payload]`).
//!
//! Content connections upgrade on `/v2/rpc` with the same `hello` handshake and
//! authenticated principal, setting [`ConnectionChannel::Content`]. Separating
//! bulk transfers onto their own stream prevents large downloads or uploads
//! from blocking control-plane RPCs such as cancellation or permission replies,
//! while reusing the existing peer WebSocket proxy transparently.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{BlobInfo, FileRoot, ThreadId, limits};

/// Current binary content-plane envelope version.
pub const CONTENT_PROTOCOL_VERSION: u8 = 1;

/// Which traffic class a WebSocket carries after `hello`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionChannel {
    /// Control plane: JSON-RPC requests, responses, and incremental watch
    /// notifications.
    #[default]
    Control,
    /// Bulk content plane: pull-based binary chunked blob/file transfers and
    /// server-rendered image thumbnails.
    Content,
}

impl ConnectionChannel {
    /// Returns whether this channel is the default [`Self::Control`] lane.
    #[must_use]
    pub const fn is_control(&self) -> bool {
        matches!(self, Self::Control)
    }
}

/// Requested representation of a stored attachment or workspace image file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentVariant {
    /// Original unmodified bytes.
    #[default]
    Original,
    /// Server-rendered raster thumbnail constrained to fit within
    /// `max_px × max_px` (`64..=2048`). Non-raster images such as SVG return
    /// original bytes unmodified.
    Preview {
        /// Maximum width and height in physical pixels.
        #[serde(rename = "maxPx")]
        max_px: u32,
    },
}

/// Client-to-server request header inside a binary content-plane frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContentRequest {
    /// Pull one chunk (at most `CONTENT_CHUNK_BYTES`) of a thread-owned
    /// attachment or its preview thumbnail.
    GetAttachment {
        /// Caller-assigned request id echoed on the reply.
        id: u64,
        /// Thread owning the attachment (membership verified on every read).
        thread: ThreadId,
        /// Attachment identifier within `thread`.
        attachment: String,
        /// Original bytes or thumbnail variant.
        #[serde(default)]
        variant: ContentVariant,
        /// Byte offset within the resolved variant stream.
        offset: u64,
        /// Maximum chunk bytes requested (clamped server-side to 256 KiB).
        len: u32,
    },
    /// Pull one chunk (at most `CONTENT_CHUNK_BYTES`) of a workspace-confined
    /// file or its preview thumbnail.
    GetFile {
        /// Caller-assigned request id echoed on the reply.
        id: u64,
        /// Workspace root confining `path`.
        root: FileRoot,
        /// Relative path inside `root`.
        path: String,
        /// Original bytes or thumbnail variant.
        #[serde(default)]
        variant: ContentVariant,
        /// Byte offset within the resolved variant stream.
        offset: u64,
        /// Maximum chunk bytes requested (clamped server-side to 256 KiB).
        len: u32,
    },
    /// Stream one contiguous payload chunk (at most 256 KiB) into an in-flight
    /// attachment upload identified by `upload`.
    UploadChunk {
        /// Caller-assigned request id echoed on the reply.
        id: u64,
        /// Connection-local upload identifier chosen by the client.
        upload: u64,
        /// Byte offset of this chunk (must equal bytes received so far).
        offset: u64,
    },
    /// Commit a streamed upload into `thread`'s attachments and return its
    /// [`BlobInfo`].
    UploadFinish {
        /// Caller-assigned request id echoed on the reply.
        id: u64,
        /// Connection-local upload identifier.
        upload: u64,
        /// Thread receiving ownership of the uploaded attachment.
        thread: ThreadId,
        /// Original filename.
        name: String,
        /// MIME media type.
        mime: String,
    },
    /// Discard an in-flight streamed upload.
    UploadAbort {
        /// Caller-assigned request id echoed on the reply.
        id: u64,
        /// Connection-local upload identifier.
        upload: u64,
    },
}

impl ContentRequest {
    /// Returns the caller-assigned correlation id.
    #[must_use]
    pub const fn id(&self) -> u64 {
        match self {
            Self::GetAttachment { id, .. }
            | Self::GetFile { id, .. }
            | Self::UploadChunk { id, .. }
            | Self::UploadFinish { id, .. }
            | Self::UploadAbort { id, .. } => *id,
        }
    }
}

/// Server-to-client response header inside a binary content-plane frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ContentResponse {
    /// One downloaded chunk; raw bytes follow in the binary payload suffix.
    Chunk {
        /// Echoed request id.
        id: u64,
        /// Total byte length of the resolved variant.
        #[serde(rename = "totalSize")]
        total_size: u64,
        /// MIME media type of the resolved variant.
        mime: String,
        /// Starting byte offset of this chunk.
        offset: u64,
        /// Whether this chunk reaches the end of the variant.
        eof: bool,
    },
    /// Acknowledgement for an [`ContentRequest::UploadChunk`] or
    /// [`ContentRequest::UploadAbort`].
    UploadAck {
        /// Echoed request id.
        id: u64,
        /// Connection-local upload identifier.
        upload: u64,
        /// Total bytes buffered so far for `upload`.
        received: u64,
    },
    /// Result of [`ContentRequest::UploadFinish`].
    Uploaded {
        /// Echoed request id.
        id: u64,
        /// Persisted content-addressed attachment metadata.
        info: BlobInfo,
    },
    /// Error reply for a failed content-plane operation.
    Error {
        /// Echoed request id (or `0` for malformed frames without an id).
        id: u64,
        /// Numeric JSON-RPC-compatible error code.
        code: i32,
        /// Human-readable error message.
        message: String,
    },
}

impl ContentResponse {
    /// Returns the echoed correlation id.
    #[must_use]
    pub const fn id(&self) -> u64 {
        match self {
            Self::Chunk { id, .. }
            | Self::UploadAck { id, .. }
            | Self::Uploaded { id, .. }
            | Self::Error { id, .. } => *id,
        }
    }
}

/// Versioned header wrapper serialized inside binary content frames.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope<T> {
    v: u8,
    #[serde(flatten)]
    body: T,
}

/// Framing errors for binary content-plane envelopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentFrameError {
    /// Frame exceeded [`limits::MAX_CONTENT_FRAME_BYTES`].
    FrameTooLarge { len: usize },
    /// Frame was shorter than the 4-byte big-endian header length prefix.
    TruncatedPrefix,
    /// Declared JSON header length exceeded
    /// [`limits::MAX_CONTENT_HEADER_BYTES`].
    HeaderTooLarge { len: usize },
    /// Frame ended before the declared JSON header finished.
    TruncatedHeader,
    /// Payload suffix exceeded [`limits::CONTENT_CHUNK_BYTES`].
    PayloadTooLarge { len: usize },
    /// Envelope version did not match [`CONTENT_PROTOCOL_VERSION`].
    UnsupportedVersion(u8),
    /// JSON encoding or decoding of the envelope header failed.
    Json(String),
}

impl fmt::Display for ContentFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameTooLarge { len } => write!(f, "content frame too large ({len} bytes)"),
            Self::TruncatedPrefix => write!(f, "content frame missing 4-byte length prefix"),
            Self::HeaderTooLarge { len } => write!(f, "content header too large ({len} bytes)"),
            Self::TruncatedHeader => write!(f, "content frame truncated inside header"),
            Self::PayloadTooLarge { len } => write!(f, "content payload too large ({len} bytes)"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported content protocol version {v}"),
            Self::Json(err) => write!(f, "invalid content header JSON: {err}"),
        }
    }
}

impl std::error::Error for ContentFrameError {}

/// Encodes a versioned binary content-plane frame: `[u32 BE
/// header_len][JSON][payload]`.
///
/// # Errors
///
/// Returns [`ContentFrameError`] if the encoded JSON header exceeds
/// [`limits::MAX_CONTENT_HEADER_BYTES`], `payload` exceeds
/// [`limits::CONTENT_CHUNK_BYTES`], or JSON serialization fails.
pub fn encode_content_frame<T: Serialize>(
    header: &T,
    payload: &[u8],
) -> Result<Vec<u8>, ContentFrameError> {
    if payload.len() > limits::CONTENT_CHUNK_BYTES {
        return Err(ContentFrameError::PayloadTooLarge { len: payload.len() });
    }
    let header_bytes = serde_json::to_vec(&Envelope {
        v: CONTENT_PROTOCOL_VERSION,
        body: header,
    })
    .map_err(|error| ContentFrameError::Json(error.to_string()))?;
    if header_bytes.len() > limits::MAX_CONTENT_HEADER_BYTES {
        return Err(ContentFrameError::HeaderTooLarge {
            len: header_bytes.len(),
        });
    }
    let header_len =
        u32::try_from(header_bytes.len()).map_err(|_| ContentFrameError::HeaderTooLarge {
            len: header_bytes.len(),
        })?;
    let mut frame = Vec::with_capacity(4 + header_bytes.len() + payload.len());
    frame.extend_from_slice(&header_len.to_be_bytes());
    frame.extend_from_slice(&header_bytes);
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Decodes a versioned binary content-plane frame and validates all header,
/// payload, and version bounds before returning `(header, payload_slice)`.
///
/// # Errors
///
/// Returns [`ContentFrameError`] on oversized frames, truncated buffers,
/// unsupported envelope versions, or invalid JSON headers.
pub fn decode_content_frame<T: DeserializeOwned>(
    frame: &[u8],
) -> Result<(T, &[u8]), ContentFrameError> {
    if frame.len() > limits::MAX_CONTENT_FRAME_BYTES {
        return Err(ContentFrameError::FrameTooLarge { len: frame.len() });
    }
    if frame.len() < 4 {
        return Err(ContentFrameError::TruncatedPrefix);
    }
    let header_len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if header_len > limits::MAX_CONTENT_HEADER_BYTES {
        return Err(ContentFrameError::HeaderTooLarge { len: header_len });
    }
    let payload_start = 4usize
        .checked_add(header_len)
        .ok_or(ContentFrameError::TruncatedHeader)?;
    if frame.len() < payload_start {
        return Err(ContentFrameError::TruncatedHeader);
    }
    let payload = &frame[payload_start..];
    if payload.len() > limits::CONTENT_CHUNK_BYTES {
        return Err(ContentFrameError::PayloadTooLarge { len: payload.len() });
    }
    let envelope: Envelope<T> = serde_json::from_slice(&frame[4..payload_start])
        .map_err(|error| ContentFrameError::Json(error.to_string()))?;
    if envelope.v != CONTENT_PROTOCOL_VERSION {
        return Err(ContentFrameError::UnsupportedVersion(envelope.v));
    }
    Ok((envelope.body, payload))
}
