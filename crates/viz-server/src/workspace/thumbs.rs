//! Stateless server-side raster thumbnail generation with header-only dimension
//! probing and decompression-bomb limits.

use std::io::Cursor;

use image::{
    ImageReader, Limits,
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
};

use crate::error::Error;

/// Standard preview dimension persisted alongside oversized image attachments.
pub(crate) const ATTACHMENT_PREVIEW_PX: u32 = 1024;
/// Sentinel stored in `blobs.preview_mime` when the original blob already fits
/// within [`ATTACHMENT_PREVIEW_PX`] and should be served directly via
/// `read_range`.
pub(crate) const PREVIEW_ORIGINAL_SENTINEL: &str = "original";
/// Header bytes read when probing image dimensions before deciding whether to
/// decode.
pub(crate) const HEADER_PROBE_BYTES: usize = 64 * viz_protocol::limits::KIB;

const MAX_DECODE_DIM: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
const MIN_THUMB_PX: u32 = 64;
const MAX_THUMB_PX: u32 = 2048;
const JPEG_QUALITY: u8 = 85;

/// Newly encoded downscaled raster thumbnail ready for persistence or slicing.
#[derive(Debug, Clone)]
pub(crate) struct RenderedPreview {
    /// MIME media type of `bytes` (`image/jpeg` for opaque, `image/png` when
    /// alpha is present).
    pub(crate) mime: String,
    /// Encoded thumbnail payload.
    pub(crate) bytes: Vec<u8>,
}

/// Clamps a requested thumbnail dimension to the server-supported range.
#[must_use]
pub(crate) const fn clamp_px(max_px: u32) -> u32 {
    if max_px < MIN_THUMB_PX {
        MIN_THUMB_PX
    } else if max_px > MAX_THUMB_PX {
        MAX_THUMB_PX
    } else {
        max_px
    }
}

/// Returns whether `mime` is a raster image that may benefit from server-side
/// downscaling (SVGs are vectors and always pass through as original bytes).
#[must_use]
pub(crate) fn is_raster_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && !mime.eq_ignore_ascii_case("image/svg+xml")
}

/// Probes image `(width, height)` from header bytes without decoding pixels.
#[must_use]
pub(crate) fn probe_dimensions(header: &[u8]) -> Option<(u32, u32)> {
    let mut reader = ImageReader::new(Cursor::new(header))
        .with_guessed_format()
        .ok()?;
    reader.limits(decode_limits());
    reader.into_dimensions().ok()
}

/// Returns `Ok(None)` when `raw` is an SVG or already fits within `max_px ×
/// max_px` (probing the header first to skip decoding entirely), or
/// `Ok(Some(..))` with a compact resized thumbnail.
pub(crate) fn render_if_larger(
    original_mime: &str,
    raw: &[u8],
    max_px: u32,
) -> Result<Option<RenderedPreview>, Error> {
    if !is_raster_image_mime(original_mime) {
        return Ok(None);
    }
    let clamped = clamp_px(max_px);
    if let Some((width, height)) = probe_dimensions(raw)
        && width <= clamped
        && height <= clamped
    {
        return Ok(None);
    }
    let mut reader = ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|error| Error::invalid(format!("couldn't sniff image format: {error}")))?;
    reader.limits(decode_limits());
    let image = reader
        .decode()
        .map_err(|error| Error::invalid(format!("couldn't decode image: {error}")))?;
    if image.width() <= clamped && image.height() <= clamped {
        return Ok(None);
    }
    let resized = image.thumbnail(clamped, clamped);
    if resized.color().has_alpha() {
        let rgba = resized.to_rgba8();
        let mut png_bytes = Vec::new();
        let encoder = PngEncoder::new_with_quality(
            &mut png_bytes,
            CompressionType::Fast,
            FilterType::Adaptive,
        );
        image::ImageEncoder::write_image(
            encoder,
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| Error::unavailable(format!("encoding png thumbnail: {error}")))?;
        Ok(Some(RenderedPreview {
            mime: "image/png".to_owned(),
            bytes: png_bytes,
        }))
    } else {
        let rgb = resized.to_rgb8();
        let mut jpeg_bytes = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_bytes, JPEG_QUALITY);
        encoder
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|error| Error::unavailable(format!("encoding jpeg thumbnail: {error}")))?;
        Ok(Some(RenderedPreview {
            mime: "image/jpeg".to_owned(),
            bytes: jpeg_bytes,
        }))
    }
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIM);
    limits.max_image_height = Some(MAX_DECODE_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}
