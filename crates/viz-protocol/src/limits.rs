//! Shared wire framing and payload size limits across server, proxy, client,
//! and desktop.

/// One kibibyte in bytes.
pub const KIB: usize = 1024;
/// One mebibyte in bytes.
pub const MIB: usize = 1024 * 1024;

/// Maximum single WebSocket frame or message size accepted by server, client,
/// and peer WebSocket proxy (`32 MiB`).
pub const MAX_WS_FRAME: usize = 32 * MIB;

/// Maximum decoded byte total for attachments on a single prompt (`20 MiB`).
pub const ATTACHMENT_TOTAL_BYTES: usize = 20 * MIB;

/// Base64-encoded ceiling corresponding to [`ATTACHMENT_TOTAL_BYTES`]
/// (`ceil(20 MiB / 3) * 4`).
pub const ATTACHMENT_ENCODED_LIMIT: usize = ATTACHMENT_TOTAL_BYTES.div_ceil(3) * 4;

/// Maximum raw bytes returned in one streamed content chunk (`256 KiB`).
pub const CONTENT_CHUNK_BYTES: usize = 256 * KIB;

/// Maximum JSON header bytes permitted in one binary content-plane envelope
/// (`8 KiB`).
pub const MAX_CONTENT_HEADER_BYTES: usize = 8 * KIB;

/// Maximum total binary content-plane frame size (`4 + 8 KiB + 256 KiB`).
pub const MAX_CONTENT_FRAME_BYTES: usize = 4 + MAX_CONTENT_HEADER_BYTES + CONTENT_CHUNK_BYTES;

/// Maximum serialized entry bytes returned in one transcript page before
/// truncating pagination (`2 MiB`).
pub const PAGE_WIRE_BYTES: usize = 2 * MIB;

/// Maximum UTF-8 bytes returned inline for a text file preview (`512 KiB`).
pub const PREVIEW_TEXT_BYTES: usize = 512 * KIB;

/// Maximum raw bytes returned inline for an image file preview (`10 MiB`).
pub const PREVIEW_IMAGE_BYTES: usize = 10 * MIB;

/// Maximum serialized entry bytes sent inline before deferring to chunk reads
/// (`16 KiB`).
pub const INLINE_ENTRY_BYTES: usize = 16 * KIB;
