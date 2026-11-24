//! Per-kind wire projection for transcript entries (never omitting entire
//! cards) and zero-cache `O(chunk)` stored entry reads via SQLite incremental
//! blob I/O.

use std::io;

use rusqlite::MAIN_DB;
use serde::Serialize;
use serde_json::Value;
use viz_protocol::{
    Body, ContentKind, Entry, EntryData, EntryId, LiveTurn, PlanContent, ThreadId, ToolCall,
    ToolContent, ToolDiff, TranscriptEntry, limits,
};

use super::{
    super::{Workspace, sql::row},
    tool_storage,
};
use crate::error::Error;

const ENTRY_BUDGET: usize = limits::ENTRY_WIRE_BUDGET;
const TEXT_CAP: usize = limits::TOOL_TEXT_WIRE_BYTES;
const DIFF_CONTEXT_LINES: usize = 3;
const DIFF_TEXT_CAP: usize = TEXT_CAP * 2;

/// Zero-allocation writer counting exact JSON byte length.
struct CountingWriter(usize);

impl io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Computes the exact serialized JSON byte length of `value` without
/// allocating an intermediate `String` or `Vec<u8>`.
pub(in crate::workspace) fn serialized_len(value: &impl Serialize) -> usize {
    let mut counter = CountingWriter(0);
    let _ = serde_json::to_writer(&mut counter, value);
    counter.0
}

fn fits(value: &impl Serialize, budget: usize) -> bool {
    serialized_len(value) <= budget
}

/// Projects `entry` for control-plane wire delivery without ever emitting an
/// omitted placeholder, setting `entry.full = Some(stored_bytes)` whenever
/// user-visible content was shortened. Returns the projected JSON byte length.
pub(in crate::workspace) fn preview(entry: &mut Entry, stored_bytes: u64) -> usize {
    let over_budget = !fits(&entry.data, ENTRY_BUDGET);
    let trimmed = match &mut entry.data {
        EntryData::Transcript(segment) => segment_preview(segment),
        EntryData::ToolUpdate { record, .. } => tool_preview(record),
        EntryData::Prompt { prompt, .. } => {
            if over_budget {
                let mut text = prompt.display_text();
                truncate_head(&mut text, TEXT_CAP);
                *prompt = viz_protocol::Prompt::new(text);
            }
            over_budget
        }
        EntryData::TaskResult { text, .. } => over_budget && truncate_head(text, TEXT_CAP),
        EntryData::Run(end) => {
            over_budget
                && end
                    .answer
                    .as_mut()
                    .is_some_and(|answer| truncate_head(answer, TEXT_CAP))
        }
        EntryData::Continued => false,
    };
    if trimmed {
        entry.full = Some(stored_bytes);
    }
    serialized_len(entry)
}

/// Projects an in-progress live turn for streaming watchers in place.
pub(in crate::workspace) fn live_preview(mut live: LiveTurn) -> LiveTurn {
    for entry in &mut live.entries {
        let _ = segment_preview(entry);
    }
    live
}

fn segment_preview(entry: &mut TranscriptEntry) -> bool {
    match &mut entry.body {
        Body::Text(_) | Body::Thought(_) => false,
        Body::Tool(tool) => tool_preview(tool),
        Body::Plan(plan) => {
            if fits(plan, ENTRY_BUDGET) {
                return false;
            }
            match &mut plan.content {
                PlanContent::Markdown { text, .. } => truncate_head(text, TEXT_CAP),
                PlanContent::Items { entries } => {
                    let original = entries.len();
                    while entries.len() > 1 && !fits(entries, ENTRY_BUDGET) {
                        entries.pop();
                    }
                    entries.len() < original
                }
            }
        }
        Body::Compaction(compaction) => {
            if fits(compaction, ENTRY_BUDGET) {
                return false;
            }
            truncate_head(&mut compaction.summary, TEXT_CAP)
        }
        Body::Media(media) => {
            if fits(media, ENTRY_BUDGET) {
                return false;
            }
            if let Some(obj) = media.block.payload.as_object_mut() {
                obj.remove("data");
                if let Some(res) = obj.get_mut("resource").and_then(Value::as_object_mut) {
                    res.remove("blob");
                    if let Some(Value::String(text)) = res.get_mut("text") {
                        return truncate_head(text, TEXT_CAP);
                    }
                }
            }
            true
        }
    }
}

fn tool_preview(tool: &mut ToolCall) -> bool {
    tool.meta = None;
    let settled = tool.status.is_settled();
    let mut trimmed = tool
        .display_output
        .as_mut()
        .is_some_and(|text| trim_for_status(text, TEXT_CAP, settled));
    for content in &mut tool.content {
        match content {
            ToolContent::Diff(diff) => trimmed |= trim_diff(diff),
            ToolContent::Block(block) => {
                if block.kind == ContentKind::Text
                    && let Some(Value::String(text)) = block.payload.get_mut("text")
                {
                    trimmed |= trim_for_status(text, TEXT_CAP, settled);
                }
            }
            ToolContent::Terminal(_) | ToolContent::Session(_) => {}
        }
    }

    for terminal in &mut tool.terminals {
        trimmed |= trim_for_status(&mut terminal.output, TEXT_CAP, settled);
    }

    tool.raw_output = None;

    if tool
        .raw_input
        .as_ref()
        .is_some_and(|raw| !fits(raw, TEXT_CAP))
    {
        let mut preview = tool.input_text().unwrap_or_else(|| {
            tool.raw_input
                .as_ref()
                .map_or_default(|raw| serde_json::to_string_pretty(raw).unwrap_or_default())
        });
        truncate_head(&mut preview, TEXT_CAP);
        tool.raw_input = Some(Value::String(preview));
        trimmed = true;
    }

    trimmed
}

/// Trims a [`ToolDiff`] to its changed line span plus [`DIFF_CONTEXT_LINES`]
/// of surrounding context, setting [`ToolDiff::skip`] to the number of leading
/// unchanged lines removed so line numbers and diff stats stay exact.
///
/// Returns `true` only when the changed region itself still had to be
/// truncated past [`DIFF_TEXT_CAP`].
fn trim_diff(diff: &mut ToolDiff) -> bool {
    if let Some(old) = diff.old_text.as_deref() {
        let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
        let new_lines: Vec<&str> = diff.new_text.split_inclusive('\n').collect();
        let mut prefix = 0usize;
        while prefix < old_lines.len()
            && prefix < new_lines.len()
            && old_lines[prefix] == new_lines[prefix]
        {
            prefix += 1;
        }
        let mut suffix = 0usize;
        while suffix < old_lines.len().saturating_sub(prefix)
            && suffix < new_lines.len().saturating_sub(prefix)
            && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
        {
            suffix += 1;
        }
        let start = prefix.saturating_sub(DIFF_CONTEXT_LINES);
        let old_end = (old_lines.len() - suffix + DIFF_CONTEXT_LINES).min(old_lines.len());
        let new_end = (new_lines.len() - suffix + DIFF_CONTEXT_LINES).min(new_lines.len());
        if start > 0 || old_end < old_lines.len() || new_end < new_lines.len() {
            let trimmed_old = old_lines[start..old_end].concat();
            let trimmed_new = new_lines[start..new_end].concat();
            diff.old_text = Some(trimmed_old);
            diff.new_text = trimmed_new;
            diff.skip = diff
                .skip
                .saturating_add(u32::try_from(start).unwrap_or(u32::MAX));
        }
    }
    let old_clipped = diff
        .old_text
        .as_mut()
        .is_some_and(|old| truncate_head(old, DIFF_TEXT_CAP));
    let new_clipped = truncate_head(&mut diff.new_text, DIFF_TEXT_CAP);
    old_clipped || new_clipped
}

fn trim_for_status(text: &mut String, limit: usize, settled: bool) -> bool {
    if settled {
        truncate_head(text, limit)
    } else {
        truncate_tail(text, limit)
    }
}

fn truncate_head(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    text.truncate(text.floor_char_boundary(limit));
    true
}

fn truncate_tail(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let cut = text.ceil_char_boundary(text.len().saturating_sub(limit));
    text.drain(..cut);
    true
}

/// Reads one binary byte range `[offset, offset + len)` directly from the
/// stored JSON column in SQLite using incremental blob I/O (`blob_open` +
/// `read_at`), returning `(bytes, total_size, rev)` in `O(chunk)` time and
/// memory with zero caching.
pub(in crate::workspace) async fn read_entry_chunk(
    workspace: &Workspace,
    thread: ThreadId,
    entry: EntryId,
    expected_rev: Option<u64>,
    offset: u64,
    len: usize,
) -> Result<(Vec<u8>, u64, u64), Error> {
    workspace
        .db
        .read_snapshot(move |tx| {
            let meta = row(
                tx,
                "SELECT rev, length(CAST(data AS BLOB)) FROM entries WHERE thread = ?1 AND id = ?2",
                [thread.into(), entry.into()],
            )?
            .ok_or_else(|| Error::not_found(format!("no entry {entry} in thread {thread}")))?;
            let rev = meta.u64(0)?;
            let total_size = meta.u64(1)?;
            if expected_rev.is_some_and(|wanted| wanted != rev) {
                return Err(Error::unavailable(
                    "entry changed between reads; restart at offset zero",
                ));
            }
            let rowid =
                i64::try_from(entry.0).map_err(|_| Error::invalid("entry id exceeds rowid"))?;
            let blob = tx.blob_open(MAIN_DB, "entries", "data", rowid, true)?;
            let mut prefix = [0; 512];
            let read = blob.read_at(&mut prefix, 0)?;
            if tool_storage::may_need_normalization(&prefix[..read]) {
                let stored = row(tx, "SELECT data FROM entries WHERE id = ?1", [entry.into()])?
                    .ok_or_else(|| {
                        Error::not_found(format!("no entry {entry} in thread {thread}"))
                    })?
                    .str(0)?;
                let data = tool_storage::decode(&stored)?;
                let projected = serde_json::to_vec(&data)?;
                let total_size = u64::try_from(projected.len()).unwrap_or(u64::MAX);
                let start = usize::try_from(offset)
                    .ok()
                    .filter(|start| *start <= projected.len())
                    .ok_or_else(|| Error::invalid("entry offset is past end of row"))?;
                let end = start.saturating_add(len).min(projected.len());
                return Ok((projected[start..end].to_vec(), total_size, rev));
            }
            if offset > total_size {
                return Err(Error::invalid("entry offset is past end of row"));
            }
            if offset == total_size || len == 0 {
                return Ok((Vec::new(), total_size, rev));
            }
            let start = usize::try_from(offset)
                .map_err(|_| Error::invalid("entry offset exceeds address space"))?;
            let remaining = usize::try_from(total_size - offset)
                .unwrap_or(usize::MAX)
                .min(len);
            let mut buf = vec![0u8; remaining];
            let read = blob.read_at(&mut buf, start)?;
            buf.truncate(read);
            Ok((buf, total_size, rev))
        })
        .await
}
