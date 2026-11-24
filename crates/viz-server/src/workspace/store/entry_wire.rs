//! Bounds transcript payloads at the wire edge and reads their full data by
//! revision through a bounded in-memory LRU chunk cache.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};
use viz_protocol::{
    Body, Entry, EntryChunk, EntryData, EntryId, EntrySource, LiveTurn, ReadEntryChunk, ThreadId,
    ToolCall, TranscriptEntry,
};

use super::{
    super::{Workspace, sql::row},
    json,
};
use crate::error::Error;

/// A single entry stays bounded before deferring to `thread.entry_chunk`.
const INLINE_BYTES: usize = viz_protocol::limits::INLINE_ENTRY_BYTES;
/// A returned JSON string stays bounded even after outer JSON escaping.
const CHUNK_BYTES: usize = viz_protocol::limits::INLINE_ENTRY_BYTES;
const CHUNK_CACHE_ENTRIES: usize = 16;
const CHUNK_CACHE_BYTES: usize = 16 * viz_protocol::limits::MIB;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CacheKey {
    Stored { thread: ThreadId, entry: EntryId },
    Live { thread: ThreadId, key: String },
}

#[derive(Debug)]
struct CachedPayload {
    revision: String,
    data: Arc<String>,
    last_used: u64,
}

#[derive(Debug, Default)]
struct CacheInner {
    entries: HashMap<CacheKey, CachedPayload>,
    total_bytes: usize,
}

/// Bounded LRU cache of serialized full entries so sequential
/// `thread.entry_chunk` pulls stay `O(chunk)` instead of re-reading SQLite and
/// re-hashing the whole entry on every chunk.
#[derive(Debug, Default)]
pub(in crate::workspace) struct EntryChunkCache {
    inner: Mutex<CacheInner>,
    clock: AtomicU64,
}

impl EntryChunkCache {
    fn get(&self, key: &CacheKey, revision: &str) -> Option<Arc<String>> {
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let found = inner
            .entries
            .get_mut(key)
            .filter(|entry| entry.revision == revision)
            .map(|entry| {
                entry.last_used = tick;
                Arc::clone(&entry.data)
            });
        drop(inner);
        found
    }

    fn insert(&self, key: CacheKey, revision: String, data: Arc<String>) {
        let size = data.len();
        if size > CHUNK_CACHE_BYTES {
            return;
        }
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(old) = inner.entries.remove(&key) {
            inner.total_bytes = inner.total_bytes.saturating_sub(old.data.len());
        }
        while (inner.entries.len() >= CHUNK_CACHE_ENTRIES
            || inner.total_bytes.saturating_add(size) > CHUNK_CACHE_BYTES)
            && let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, _)| k.clone())
        {
            if let Some(evicted) = inner.entries.remove(&oldest) {
                inner.total_bytes = inner.total_bytes.saturating_sub(evicted.data.len());
            }
        }
        inner.total_bytes = inner.total_bytes.saturating_add(size);
        inner.entries.insert(
            key,
            CachedPayload {
                revision,
                data,
                last_used: tick,
            },
        );
    }

    /// Invalidates a cached stored entry after a tool patch mutates its JSON.
    pub(in crate::workspace) fn invalidate_stored(&self, thread: ThreadId, entry: EntryId) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(removed) = inner.entries.remove(&CacheKey::Stored { thread, entry }) {
            inner.total_bytes = inner.total_bytes.saturating_sub(removed.data.len());
        }
    }
}

/// Trims oversized entry payloads for wire transmission and returns the
/// resulting serialized byte size for page budgeting.
///
/// Oversized `Prompt` entries truncate their `text` and set `truncated = true`
/// while preserving `attachments`, `context`, and `origin` metadata so the UI
/// never loses pills or attachments when long prompts defer to chunk loading.
pub(in crate::workspace) fn preview(entry: &mut Entry) -> Result<usize, Error> {
    match &mut entry.data {
        EntryData::Transcript(segment) => segment_preview(segment),
        EntryData::ToolUpdate { record, .. } => tool_preview(record),
        EntryData::Prompt { .. } => {
            if json(&entry.data)?.len() > INLINE_BYTES {
                if let EntryData::Prompt {
                    text, truncated, ..
                } = &mut entry.data
                {
                    truncate(text);
                    *truncated = true;
                }
                if json(&entry.data)?.len() > INLINE_BYTES {
                    entry.data = EntryData::Omitted;
                }
            }
        }
        data if json(data)?.len() > INLINE_BYTES => *data = EntryData::Omitted,
        _ => {}
    }
    Ok(json(entry)?.len())
}

pub(in crate::workspace) fn live_preview(mut live: LiveTurn) -> LiveTurn {
    for entry in &mut live.entries {
        segment_preview(entry);
    }
    live
}

fn segment_preview(entry: &mut TranscriptEntry) {
    if fits(&entry.body) {
        return;
    }
    entry.more = true;
    match &mut entry.body {
        Body::Tool(tool) => tool_preview(tool),
        Body::Text(segment) | Body::Thought(segment) => truncate(&mut segment.text),
        _ => entry.body = Body::Omitted,
    }
    if !fits(&entry.body) {
        entry.body = Body::Omitted;
    }
}

fn tool_preview(tool: &mut ToolCall) {
    if fits(tool) {
        return;
    }
    let input = tool.input_text();
    let output = tool.output_text();
    let mut summary = ToolCall::new(std::mem::take(&mut tool.id));
    summary.title = std::mem::take(&mut tool.title);
    truncate(&mut summary.title);
    summary.name = tool.name.take();
    summary.kind = tool.kind.take();
    summary.viz = tool.viz.take();
    summary.exit = tool.exit.take();
    summary.status = tool.status;
    summary.started_at = tool.started_at;
    summary.ended_at = tool.ended_at;
    summary.raw_input = input.map(preview_text);
    summary.raw_output = output.map(preview_text);
    summary.more = true;
    if !fits(&summary) {
        summary.viz = None;
        summary.name = None;
        summary.kind = None;
        summary.raw_input = None;
        summary.raw_output = None;
    }
    *tool = summary;
}

fn fits(data: &impl serde::Serialize) -> bool {
    serde_json::to_vec(data).is_ok_and(|bytes| bytes.len() <= INLINE_BYTES)
}

fn preview_text(mut text: String) -> serde_json::Value {
    truncate(&mut text);
    text.into()
}

fn truncate(text: &mut String) {
    text.truncate(text.floor_char_boundary(2048));
}

pub(in crate::workspace) async fn chunk(
    workspace: &Workspace,
    request: ReadEntryChunk,
) -> Result<EntryChunk, Error> {
    let ReadEntryChunk {
        thread,
        source,
        offset,
        revision,
    } = request;
    let cache_key = match &source {
        EntrySource::Stored { entry } => CacheKey::Stored {
            thread,
            entry: *entry,
        },
        EntrySource::Live { key } => CacheKey::Live {
            thread,
            key: key.clone(),
        },
    };
    let (actual, data) = if let Some(ref wanted) = revision
        && let Some(cached) = workspace.entry_chunks.get(&cache_key, wanted)
    {
        (wanted.clone(), cached)
    } else {
        let raw = match source {
            EntrySource::Stored { entry } => {
                workspace
                    .db
                    .read(move |conn| {
                        row(
                            conn,
                            "SELECT data FROM entries WHERE thread = ?1 AND id = ?2",
                            [thread.into(), entry.into()],
                        )?
                        .ok_or_else(|| {
                            Error::not_found(format!("no entry {entry} in thread {thread}"))
                        })?
                        .str(0)
                    })
                    .await?
            }
            EntrySource::Live { key } => {
                let live = workspace.updates.raw_live(thread).ok_or_else(|| {
                    Error::not_found(format!("no live transcript in thread {thread}"))
                })?;
                let entry = live
                    .entries
                    .iter()
                    .find(|entry| entry.key == key)
                    .ok_or_else(|| {
                        Error::not_found(format!("no live entry {key} in thread {thread}"))
                    })?;
                json(entry)?
            }
        };
        let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(raw.as_bytes()));
        if revision.is_some_and(|revision| revision != actual) {
            return Err(Error::unavailable(
                "entry changed between reads; restart at offset zero",
            ));
        }
        let shared = Arc::new(raw);
        workspace
            .entry_chunks
            .insert(cache_key, actual.clone(), Arc::clone(&shared));
        (actual, shared)
    };
    let start = usize::try_from(offset).map_err(|_| Error::invalid("invalid entry offset"))?;
    if !data.is_char_boundary(start) {
        return Err(Error::invalid(
            "entry offset is outside the JSON or splits a UTF-8 character",
        ));
    }
    let end = data.floor_char_boundary((start + CHUNK_BYTES).min(data.len()));
    Ok(EntryChunk {
        revision: actual,
        data: data[start..end].to_owned(),
        next: (end < data.len()).then_some(end as u64),
    })
}
