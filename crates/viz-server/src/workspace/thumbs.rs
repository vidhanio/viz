//! Server-side bounded LRU raster thumbnail generator with decompression-bomb
//! limits.

use std::{
    collections::HashMap,
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use image::{
    ImageReader, Limits,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};

use crate::error::Error;

const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 16 * viz_protocol::limits::MIB;
const MAX_DECODE_DIM: u32 = 16_384;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
const MIN_THUMB_PX: u32 = 64;
const MAX_THUMB_PX: u32 = 2048;

/// Rendered thumbnail or pass-through image bytes.
#[derive(Debug, Clone)]
pub(crate) struct RenderedImage {
    /// MIME media type of `bytes`.
    pub(crate) mime: String,
    /// Immutable image payload ready for chunked slicing.
    pub(crate) bytes: Arc<Vec<u8>>,
}

#[derive(Debug)]
struct Entry {
    image: RenderedImage,
    last_used: u64,
}

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<(String, u32), Entry>,
    total_bytes: usize,
}

/// Bounded in-memory LRU cache of server-rendered preview thumbnails.
#[derive(Debug, Default)]
pub(crate) struct ThumbCache {
    inner: Mutex<Inner>,
    clock: AtomicU64,
}

impl ThumbCache {
    /// Returns a cached thumbnail for `(key, max_px)` if already rendered.
    pub(crate) fn get(&self, key: &str, max_px: u32) -> Option<RenderedImage> {
        let clamped = max_px.clamp(MIN_THUMB_PX, MAX_THUMB_PX);
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = inner
            .entries
            .get_mut(&(key.to_owned(), clamped))
            .map(|entry| {
                entry.last_used = tick;
                entry.image.clone()
            });
        drop(inner);
        found
    }

    /// Renders (or returns cached) a preview thumbnail fitting within
    /// `max_px × max_px`. Vector SVG inputs pass through unchanged.
    pub(crate) async fn render(
        &self,
        key: String,
        original_mime: String,
        raw: Vec<u8>,
        max_px: u32,
    ) -> Result<RenderedImage, Error> {
        let clamped = max_px.clamp(MIN_THUMB_PX, MAX_THUMB_PX);
        if let Some(hit) = self.get(&key, clamped) {
            return Ok(hit);
        }
        let rendered = if original_mime.eq_ignore_ascii_case("image/svg+xml") {
            RenderedImage {
                mime: original_mime,
                bytes: Arc::new(raw),
            }
        } else {
            tokio::task::spawn_blocking(move || render_raster(&original_mime, raw, clamped))
                .await
                .map_err(|error| {
                    Error::unavailable(format!("thumbnail worker failed: {error}"))
                })??
        };
        self.insert(key, clamped, rendered.clone());
        Ok(rendered)
    }

    fn insert(&self, key: String, max_px: u32, image: RenderedImage) {
        let size = image.bytes.len();
        if size > MAX_BYTES {
            return;
        }
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(old) = inner.entries.remove(&(key.clone(), max_px)) {
            inner.total_bytes = inner.total_bytes.saturating_sub(old.image.bytes.len());
        }
        while (inner.entries.len() >= MAX_ENTRIES
            || inner.total_bytes.saturating_add(size) > MAX_BYTES)
            && let Some(oldest_key) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, _)| k.clone())
        {
            if let Some(evicted) = inner.entries.remove(&oldest_key) {
                inner.total_bytes = inner.total_bytes.saturating_sub(evicted.image.bytes.len());
            }
        }
        inner.total_bytes = inner.total_bytes.saturating_add(size);
        inner.entries.insert(
            (key, max_px),
            Entry {
                image,
                last_used: tick,
            },
        );
    }
}

fn render_raster(original_mime: &str, raw: Vec<u8>, max_px: u32) -> Result<RenderedImage, Error> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIM);
    limits.max_image_height = Some(MAX_DECODE_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);

    let mut reader = ImageReader::new(Cursor::new(&raw))
        .with_guessed_format()
        .map_err(|error| Error::invalid(format!("couldn't sniff image format: {error}")))?;
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| Error::invalid(format!("couldn't decode image: {error}")))?;
    if image.width() <= max_px && image.height() <= max_px {
        return Ok(RenderedImage {
            mime: original_mime.to_owned(),
            bytes: Arc::new(raw),
        });
    }
    let resized = image.thumbnail(max_px, max_px);
    let rgba = resized.to_rgba8();
    let mut png_bytes = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Fast, FilterType::Adaptive);
    image::ImageEncoder::write_image(
        encoder,
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|error| Error::unavailable(format!("encoding thumbnail: {error}")))?;
    Ok(RenderedImage {
        mime: "image/png".to_owned(),
        bytes: Arc::new(png_bytes),
    })
}
