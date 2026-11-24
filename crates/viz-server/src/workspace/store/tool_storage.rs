//! Derives current tool fields when reading pre-normalization SQLite rows.

use serde_json::{Value, json};
use viz_protocol::{Body, EntryData, ToolContent, ToolDiff, TranscriptEntry};

use super::from_json;
use crate::error::Error;

/// Identifies historical tool rows from their serialized envelope. With a
/// partial blob, a long transcript key can defer the decision to a full read.
pub(super) fn may_need_normalization(bytes: &[u8]) -> bool {
    const TOOL: &[u8] = br#"tool","data":{"#;
    if bytes.starts_with(br#"{"kind":"transcript","data":{"#) {
        let Some(start) = locate(bytes, br#""body":{"type":""#) else {
            return true;
        };
        let body = &bytes[start..];
        if let Some(call) = body.strip_prefix(TOOL) {
            return !call.starts_with(br#""payload":"#);
        }
        return TOOL.starts_with(body);
    }
    if bytes.starts_with(br#"{"kind":"tool_update","data":{"#) {
        return locate(bytes, br#""record":{"#)
            .is_none_or(|start| !bytes[start..].starts_with(br#""payload":"#));
    }
    false
}

fn locate(bytes: &[u8], marker: &[u8]) -> Option<usize> {
    bytes
        .windows(marker.len())
        .position(|part| part == marker)
        .map(|pos| pos + marker.len())
}

pub(super) fn decode(text: &str) -> Result<EntryData, Error> {
    if !may_need_normalization(text.as_bytes()) {
        return from_json(text);
    }
    let mut value: Value = from_json(text)?;
    let pointer = match value.get("kind").and_then(Value::as_str) {
        Some("transcript")
            if value.pointer("/data/body/type").and_then(Value::as_str) == Some("tool") =>
        {
            "/data/body/data"
        }
        Some("tool_update") => "/data/record",
        _ => return serde_json::from_value(value).map_err(Error::corrupt),
    };
    let Some(tool) = value.pointer_mut(pointer).and_then(Value::as_object_mut) else {
        return serde_json::from_value(value).map_err(Error::corrupt);
    };
    if tool.contains_key("payload") {
        return serde_json::from_value(value).map_err(Error::corrupt);
    }
    tool.insert("payload".into(), json!({"type": "other", "summary": null}));
    if let Some(content) = tool.get_mut("content").and_then(Value::as_array_mut) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("diff")
                && let Some(diff) = item.get_mut("data").and_then(Value::as_object_mut)
            {
                diff.insert("added".into(), json!(0));
                diff.insert("removed".into(), json!(0));
            }
        }
    }
    let mut data: EntryData = serde_json::from_value(value).map_err(Error::corrupt)?;
    let (EntryData::Transcript(TranscriptEntry {
        body: Body::Tool(call),
        ..
    })
    | EntryData::ToolUpdate { record: call, .. }) = &mut data
    else {
        return Ok(data);
    };
    for content in &mut call.content {
        if let ToolContent::Diff(diff) = content {
            let skip = diff.skip;
            *diff = ToolDiff::new(
                std::mem::take(&mut diff.path),
                diff.old_text.take(),
                std::mem::take(&mut diff.new_text),
            );
            diff.skip = skip;
        }
    }
    viz_acp::normalize_stored_tool(call);
    Ok(data)
}
