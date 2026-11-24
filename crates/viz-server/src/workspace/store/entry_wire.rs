//! Bounds transcript payloads at the wire edge and reads their full data by
//! revision.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};
use viz_protocol::{
    Body, Entry, EntryChunk, EntryData, EntrySource, LiveTurn, ReadEntryChunk, ToolCall,
    TranscriptEntry,
};

use super::{
    super::{Workspace, sql::row},
    json,
};
use crate::error::Error;

/// A 200-entry page stays below typical host message limits.
const INLINE_BYTES: usize = viz_protocol::limits::INLINE_ENTRY_BYTES;
/// A returned JSON string stays bounded even after outer JSON escaping.
const CHUNK_BYTES: usize = viz_protocol::limits::INLINE_ENTRY_BYTES;

pub(in crate::workspace) fn preview(entry: &mut Entry) -> Result<(), Error> {
    match &mut entry.data {
        EntryData::Transcript(segment) => segment_preview(segment),
        EntryData::ToolUpdate { record, .. } => tool_preview(record),
        data if json(data)?.len() > INLINE_BYTES => *data = EntryData::Omitted,
        _ => {}
    }
    Ok(())
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
    let data = match source {
        EntrySource::Stored { entry } => {
            let mut db = workspace.db.lock().await;
            row(
                &mut db,
                "SELECT data FROM entries WHERE thread = ?1 AND id = ?2",
                [thread.into(), entry.into()],
            )
            .await?
            .ok_or_else(|| Error::not_found(format!("no entry {entry} in thread {thread}")))?
            .str(0)?
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
    let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(data.as_bytes()));
    if revision.is_some_and(|revision| revision != actual) {
        return Err(Error::unavailable(
            "entry changed between reads; restart at offset zero",
        ));
    }
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
