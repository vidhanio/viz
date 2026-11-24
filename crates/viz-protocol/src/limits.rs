//! Shared wire framing and payload size limits across server, proxy, client,
//! and desktop.

/// One kibibyte in bytes.
pub const KIB: usize = 1024;
/// One mebibyte in bytes.
pub const MIB: usize = 1024 * 1024;

/// Maximum single WebSocket frame or message size (`2 MiB`).
///
/// Bulk attachments, previews, icons, and full entries travel as 256 KiB binary
/// chunks on the dedicated content WebSocket.
pub const MAX_WS_FRAME: usize = 2 * MIB;

/// Maximum decoded byte total for attachments on a single prompt (`20 MiB`).
pub const ATTACHMENT_TOTAL_BYTES: usize = 20 * MIB;

/// Maximum raw bytes returned in one streamed content chunk (`256 KiB`).
pub const CONTENT_CHUNK_BYTES: usize = 256 * KIB;

/// Maximum JSON header bytes permitted in one binary content-plane envelope
/// (`8 KiB`).
pub const MAX_CONTENT_HEADER_BYTES: usize = 8 * KIB;

/// Maximum total binary content-plane frame size (`4 + 8 KiB + 256 KiB`).
pub const MAX_CONTENT_FRAME_BYTES: usize = 4 + MAX_CONTENT_HEADER_BYTES + CONTENT_CHUNK_BYTES;

/// Maximum serialized entry bytes returned in one transcript page
/// (`MAX_WS_FRAME - 128 KiB`).
///
/// Leaves headroom for JSON-RPC framing and array separators so a full page
/// never exceeds [`MAX_WS_FRAME`].
pub const PAGE_WIRE_BYTES: usize = MAX_WS_FRAME - 128 * KIB;

/// Target byte budget for the initial transcript page sent when opening or
/// watching a thread (`256 KiB`), keeping thread switches fast while history
/// prefetches seamlessly on scroll.
pub const FIRST_PAGE_WIRE_BYTES: usize = 256 * KIB;

/// Maximum serialized bytes for a single entry projected onto the control plane
/// before per-kind trimming engages (`64 KiB`).
pub const ENTRY_WIRE_BUDGET: usize = 64 * KIB;

/// Per-field text ceiling (`16 KiB`) applied when trimming oversized prompts,
/// plans, compactions, task results, or tool inputs/outputs on the wire.
pub const TOOL_TEXT_WIRE_BYTES: usize = 16 * KIB;

/// Stored entry size threshold (`1 MiB`) below which clients hydrate trimmed
/// entries automatically over the content plane without asking.
pub const AUTO_HYDRATE_ENTRY_BYTES: u64 = MIB as u64;

/// Maximum UTF-8 bytes returned inline for a text file preview (`512 KiB`).
pub const PREVIEW_TEXT_BYTES: usize = 512 * KIB;

/// Maximum raw bytes returned inline for an image file preview (`10 MiB`).
pub const PREVIEW_IMAGE_BYTES: usize = 10 * MIB;
