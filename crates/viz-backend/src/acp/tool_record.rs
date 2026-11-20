//! ACP tool call updates folded into [`ToolRecord`]s, and the receipts that
//! link a call to the viz tool it ran.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use agent_client_protocol::v1::schema::{
    ContentBlock, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus as AcpStatus,
    ToolCallUpdateFields, ToolKind,
};
use serde_json::Value;
use uuid::Uuid;

use crate::wire::{ThreadId, ToolCallStatus, ToolDiff, ToolRecord};

/// Characters kept of a tool's input.
const INPUT_LIMIT: usize = 4000;
/// Characters kept of a tool's displayed output.
const OUTPUT_LIMIT: usize = 8000;
/// Characters kept of each side of a diff.
const DIFF_LIMIT: usize = 20_000;

const RECEIPT_PREFIX: &str = "[viz-tool:";
/// A hyphenated UUID's length.
const RECEIPT_ID_LEN: usize = 36;

const OUTPUT_FIELDS: [&str; 6] = [
    "formatted_output",
    "aggregated_output",
    "output",
    "content",
    "text",
    "result",
];

/// One tool call's fields, from its first `tool_call` or a later
/// `tool_call_update`.
pub(super) struct ToolFields {
    title: Option<String>,
    name: Option<String>,
    kind: Option<ToolKind>,
    status: Option<AcpStatus>,
    content: Option<Vec<ToolCallContent>>,
    locations: Option<Vec<ToolCallLocation>>,
    raw_input: Option<Value>,
    raw_output: Option<Value>,
}

impl From<ToolCall> for ToolFields {
    fn from(call: ToolCall) -> Self {
        let ToolCall {
            title,
            name,
            kind,
            status,
            content,
            locations,
            raw_input,
            raw_output,
            ..
        } = call;
        Self {
            title: Some(title),
            name,
            kind: Some(kind),
            status: Some(status),
            content: Some(content),
            locations: Some(locations),
            raw_input,
            raw_output,
        }
    }
}

impl From<ToolCallUpdateFields> for ToolFields {
    fn from(update: ToolCallUpdateFields) -> Self {
        let ToolCallUpdateFields {
            kind,
            status,
            title,
            name,
            content,
            locations,
            raw_input,
            raw_output,
            ..
        } = update;
        Self {
            title,
            name,
            kind,
            status,
            content,
            locations,
            raw_input,
            raw_output,
        }
    }
}

/// ACP and MCP do not share call IDs: viz correlates the MCP response itself,
/// not names, titles, or request order. A viz tool's response carries a marker
/// naming its receipt; the ACP tool call whose output shows it claims it.
#[derive(Debug, Default)]
pub(crate) struct Receipts {
    calls: Mutex<HashMap<String, Call>>,
}

#[derive(Debug)]
struct Call {
    viz: String,
    child: Option<u64>,
    child_environment: Option<String>,
    claimed_by: Option<String>,
}

impl Receipts {
    /// A marker for a viz tool's response text. Some runners only forward MCP
    /// text blocks, not structured metadata.
    pub(crate) fn issue(
        &self,
        viz: &str,
        child: Option<u64>,
        child_environment: Option<String>,
    ) -> String {
        let id = Uuid::new_v4().hyphenated().to_string();
        let call = Call {
            viz: viz.to_owned(),
            child,
            child_environment,
            claimed_by: None,
        };
        self.calls
            .lock()
            .expect("receipts lock poisoned")
            .insert(id.clone(), call);
        format!("{RECEIPT_PREFIX}{id}]")
    }

    /// `value` without this turn's receipt markers; the first record to show a
    /// receipt claims its call.
    fn strip(&self, value: &str, record: &mut ToolRecord) -> String {
        let mut calls = self.calls.lock().expect("receipts lock poisoned");
        if calls.is_empty() {
            return value.to_owned();
        }
        let mut result = String::with_capacity(value.len());
        let mut rest = value;
        while let Some(start) = rest.find(RECEIPT_PREFIX) {
            let after = &rest[start + RECEIPT_PREFIX.len()..];
            let id = after.get(..RECEIPT_ID_LEN).filter(|id| {
                id.bytes().all(|byte| {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-'
                }) && after[RECEIPT_ID_LEN..].starts_with(']')
            });
            let call = id.and_then(|id| calls.get_mut(id));
            let Some(call) = call else {
                result.push_str(&rest[..start + RECEIPT_PREFIX.len()]);
                rest = after;
                continue;
            };
            result.push_str(&rest[..start]);
            rest = &after[RECEIPT_ID_LEN + 1..];
            if call
                .claimed_by
                .as_ref()
                .is_none_or(|claimant| *claimant == record.id)
            {
                call.claimed_by = Some(record.id.clone());
                record.viz = Some(call.viz.clone());
                if let Some(child) = call.child {
                    record.child = Some(ThreadId(child));
                }
                if let Some(environment) = &call.child_environment {
                    record.child_environment = Some(environment.clone());
                }
            }
        }
        drop(calls);
        result.push_str(rest);
        result
    }
}

/// One tool call's record, built from its updates in order.
pub(super) struct ToolContent {
    record: ToolRecord,
    raw_output: String,
    content_text: String,
}

impl ToolContent {
    pub(super) fn new(id: String) -> Self {
        let record = ToolRecord {
            title: id.clone(),
            id,
            name: None,
            kind: None,
            status: ToolCallStatus::Pending,
            viz: None,
            child: None,
            child_environment: None,
            started_at: None,
            input: None,
            output: None,
            diffs: Vec::new(),
            locations: Vec::new(),
        };
        Self {
            record,
            raw_output: String::new(),
            content_text: String::new(),
        }
    }

    pub(super) const fn record(&self) -> &ToolRecord {
        &self.record
    }

    pub(super) fn update(&mut self, fields: ToolFields, receipts: &Receipts) -> &ToolRecord {
        let record = &mut self.record;
        if let Some(title) = fields.title {
            record.title = title;
        }
        if fields.name.is_some() {
            record.name = fields.name;
        }
        // An unreported kind deserializes as `other`; keep what an earlier
        // update said.
        if let Some(kind) = fields.kind.filter(|kind| *kind != ToolKind::Other) {
            record.kind = serde_json::to_value(kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned));
        }
        if let Some(status) = fields.status {
            record.status = match status {
                AcpStatus::InProgress => ToolCallStatus::InProgress,
                AcpStatus::Completed => ToolCallStatus::Completed,
                AcpStatus::Failed => ToolCallStatus::Failed,
                _ => ToolCallStatus::Pending,
            };
        }
        if record.status == ToolCallStatus::InProgress && record.started_at.is_none() {
            record.started_at = Some(now_millis());
        }
        if let Some(input) = fields.raw_input.filter(|value| !value.is_null()) {
            let input = input_text(&input);
            record.input = (!input.is_empty()).then_some(input);
        }
        if let Some(output) = fields.raw_output.filter(|value| !value.is_null()) {
            let raw = raw_text(&output, 0).unwrap_or_else(|| pretty(&output));
            self.raw_output = truncate(&receipts.strip(&raw, record), OUTPUT_LIMIT);
        }
        if let Some(content) = fields.content {
            self.content_text.clear();
            let mut diffs = Vec::new();
            for item in content {
                match item {
                    ToolCallContent::Diff(diff) => diffs.push(ToolDiff {
                        path: diff.path.display().to_string(),
                        old_text: diff.old_text.map(|text| truncate(&text, DIFF_LIMIT)),
                        new_text: truncate(&diff.new_text, DIFF_LIMIT),
                    }),
                    ToolCallContent::Content(content) => {
                        let ContentBlock::Text(text) = content.content else {
                            continue;
                        };
                        let text = receipts.strip(&text.text, record);
                        let used = self.content_text.chars().count();
                        if text.is_empty() || used >= OUTPUT_LIMIT {
                            continue;
                        }
                        let separator = if self.content_text.is_empty() {
                            ""
                        } else {
                            "\n"
                        };
                        self.content_text.push_str(separator);
                        self.content_text
                            .push_str(&truncate(&text, OUTPUT_LIMIT - used - separator.len()));
                    }
                    _ => {}
                }
            }
            record.diffs = diffs;
        }
        if let Some(locations) = fields.locations {
            let mut paths: Vec<String> = Vec::with_capacity(locations.len());
            for location in locations {
                let path = location.path.display().to_string();
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
            record.locations = paths;
        }
        // ACP content is the display text; raw output is runner metadata, shown
        // only when there is no content.
        let mut output = if self.content_text.is_empty() {
            self.raw_output.trim()
        } else {
            self.content_text.as_str()
        };
        // omp's execute content starts with an echo of its `$ …` title, even
        // before stdout exists.
        if record.kind.as_deref() == Some("execute") && record.title.starts_with("$ ") {
            if output == record.title {
                output = "";
            } else if let Some(rest) = output
                .strip_prefix(record.title.as_str())
                .and_then(|rest| rest.strip_prefix('\n'))
            {
                output = rest;
            }
        }
        record.output = (!output.is_empty()).then(|| output.to_owned());
        record
    }
}

/// Structured input stays JSON, so clients can read fields such as `command`,
/// `path`, or `timeout`.
fn input_text(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        value => pretty(value),
    };
    truncate(&text, INPUT_LIMIT)
}

/// Readable text from ACP `rawOutput`, whose shape is runner-specific: a
/// string, text content blocks, a known text field, or `stdout`/`stderr`;
/// `None` for anything else.
fn raw_text(value: &Value, depth: u8) -> Option<String> {
    match value {
        Value::Null => Some(String::new()),
        Value::String(text) => Some(text.clone()),
        _ if depth > 2 => None,
        Value::Array(items) => items
            .iter()
            .map(|item| raw_text(item, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join("\n")),
        Value::Object(record) => {
            let field = OUTPUT_FIELDS.iter().find_map(|field| {
                record
                    .get(*field)
                    .and_then(|value| raw_text(value, depth + 1))
                    .filter(|text| !text.is_empty())
            });
            field.or_else(|| {
                let streams: Vec<&str> = ["stdout", "stderr"]
                    .iter()
                    .filter_map(|name| record.get(*name).and_then(Value::as_str))
                    .filter(|text| !text.is_empty())
                    .collect();
                (!streams.is_empty()).then(|| streams.join("\n"))
            })
        }
        Value::Bool(_) | Value::Number(_) => None,
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// The first `limit` characters of `text`.
fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

fn now_millis() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
