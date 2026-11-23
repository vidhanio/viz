//! Folds ACP tool call reports into a [`ToolCall`].

use std::sync::Arc;

use serde_json::Value;

use super::{Exit, FileDiff, Location, Terminal, ToolCall, ToolContent, ToolStatus};
use crate::{
    receipts::{ReceiptBook, Receipts},
    schema::{
        self, ContentBlock, Meta, TerminalId, ToolCallContent, ToolCallLocation,
        ToolCallStatus as AcpStatus, ToolCallUpdate, ToolKind,
    },
};

/// Bytes kept of a streamed terminal's output; the start is dropped first.
const TERMINAL_LIMIT: usize = 64 * 1024;
/// Metadata that streams a terminal and is not kept as metadata.
const TERMINAL_META: [&str; 4] = [
    "terminal_info",
    "terminal_output",
    "terminal_output_delta",
    "terminal_exit",
];

/// One report's fields, from a `tool_call` or a `tool_call_update`.
#[derive(Debug, Default)]
pub struct ToolFields {
    title: Option<String>,
    name: Option<String>,
    kind: Option<ToolKind>,
    status: Option<AcpStatus>,
    content: Option<Vec<ToolCallContent>>,
    locations: Option<Vec<ToolCallLocation>>,
    raw_input: Option<Value>,
    raw_output: Option<Value>,
    meta: Option<Meta>,
}

impl ToolFields {
    pub(crate) const fn status(&self) -> Option<AcpStatus> {
        self.status
    }
}

impl From<schema::ToolCall> for ToolFields {
    fn from(call: schema::ToolCall) -> Self {
        Self {
            title: Some(call.title),
            name: call.name,
            kind: Some(call.kind),
            status: Some(call.status),
            content: Some(call.content),
            locations: Some(call.locations),
            raw_input: call.raw_input,
            raw_output: call.raw_output,
            meta: call.meta,
        }
    }
}

impl From<ToolCallUpdate> for ToolFields {
    fn from(update: ToolCallUpdate) -> Self {
        let fields = update.fields;
        Self {
            title: fields.title,
            name: fields.name,
            kind: fields.kind,
            status: fields.status,
            content: fields.content,
            locations: fields.locations,
            raw_input: fields.raw_input,
            raw_output: fields.raw_output,
            meta: update.meta,
        }
    }
}

/// A report for a call the fold no longer holds, to apply to the record the
/// caller stored.
#[derive(Debug)]
pub struct ToolPatch {
    pub id: String,
    pub(crate) fields: ToolFields,
    pub(crate) receipts: Arc<Receipts>,
    pub(crate) observed_at: i64,
}

impl ToolPatch {
    /// The stored call with this report applied.
    #[must_use]
    pub fn apply(self, mut call: ToolCall) -> ToolCall {
        self.receipts
            .with_book(|book| call.report(self.fields, book, self.observed_at));
        call
    }
}

impl ToolCall {
    /// Applies one report.
    pub(crate) fn report(
        &mut self,
        fields: ToolFields,
        receipts: &mut ReceiptBook,
        observed_at: i64,
    ) {
        if let Some(title) = fields.title {
            self.title = title;
        }
        if fields.name.is_some() {
            self.name = fields.name;
        }
        if let Some(kind) = fields.kind {
            self.kind = Some(kind);
        }
        if let Some(status) = fields.status {
            self.set_status(
                match status {
                    AcpStatus::InProgress => ToolStatus::InProgress,
                    AcpStatus::Completed => ToolStatus::Completed,
                    AcpStatus::Failed => ToolStatus::Failed,
                    _ => ToolStatus::Pending,
                },
                observed_at,
            );
        }
        self.started_at.get_or_insert(observed_at);
        if let Some(input) = fields.raw_input {
            self.raw_input = Some(input);
        }
        if let Some(mut output) = fields.raw_output {
            if let Some(link) = receipts.strip_value(&mut output, &self.id) {
                self.viz = Some(link);
            }
            if let Some(code) = exit_code(&output) {
                self.exit = Some(Exit {
                    code: Some(code),
                    signal: None,
                });
            }
            self.raw_output = Some(output);
        }
        if let Some(content) = fields.content {
            self.replace_content(content, receipts);
        }
        if let Some(meta) = fields.meta {
            self.read_meta(meta, receipts);
        }
        if let Some(locations) = fields.locations {
            self.locations = locations
                .into_iter()
                .map(|location| Location {
                    path: location.path,
                    line: location.line,
                })
                .collect();
        }
        self.read_review();
    }

    pub(crate) fn set_status(&mut self, status: ToolStatus, observed_at: i64) {
        if status == ToolStatus::InProgress && self.status == ToolStatus::Pending {
            self.started_at = Some(observed_at);
        }
        if status.is_settled() {
            self.ended_at.get_or_insert(observed_at);
        } else {
            self.ended_at = None;
        }
        self.status = status;
    }

    fn replace_content(&mut self, content: Vec<ToolCallContent>, receipts: &mut ReceiptBook) {
        self.content.clear();
        for item in content {
            let item = match item {
                ToolCallContent::Diff(diff) => ToolContent::Diff(FileDiff {
                    path: diff.path,
                    old_text: diff.old_text,
                    new_text: diff.new_text,
                }),
                ToolCallContent::Content(content) => match content.content {
                    ContentBlock::Text(mut text) => {
                        let (stripped, link) = receipts.strip(&text.text, &self.id);
                        if link.is_some() {
                            self.viz = link;
                        }
                        text.text = stripped;
                        ToolContent::Block(Box::new(ContentBlock::Text(text)))
                    }
                    block => ToolContent::Block(Box::new(block)),
                },
                ToolCallContent::Terminal(terminal) => {
                    self.terminal(&terminal.terminal_id);
                    ToolContent::Terminal(terminal.terminal_id)
                }
                ToolCallContent::Session(session) => {
                    ToolContent::Session(session.session_id.0.to_string())
                }
                _ => continue,
            };
            self.content.push(item);
        }
    }

    fn terminal(&mut self, id: &TerminalId) -> &mut Terminal {
        let index = self
            .terminals
            .iter()
            .position(|terminal| terminal.id == *id)
            .unwrap_or_else(|| {
                self.terminals.push(Terminal {
                    id: id.clone(),
                    output: String::new(),
                    truncated: false,
                    exit: None,
                });
                self.terminals.len() - 1
            });
        &mut self.terminals[index]
    }

    /// Keeps `meta`, reading the caller of a subagent's call and the terminal
    /// stream that harnesses send there (`terminal_output` for a shown
    /// terminal, `terminal_output_delta` for other commands).
    fn read_meta(&mut self, mut meta: Meta, receipts: &mut ReceiptBook) {
        let parent = meta
            .get("claudeCode")
            .and_then(|meta| meta.get("parentToolUseId"))
            .or_else(|| meta.get("parentToolCallId"))
            .and_then(Value::as_str);
        if let Some(parent) = parent {
            self.parent = Some(parent.to_owned());
        }
        let [info, output, delta, exit] = TERMINAL_META.map(|key| meta.remove(key));
        if let Some(id) = info.as_ref().and_then(terminal_id) {
            self.terminal(&id);
        }
        for chunk in [output, delta].into_iter().flatten() {
            let Some(data) = chunk.get("data").and_then(Value::as_str) else {
                continue;
            };
            let (data, link) = receipts.strip(data, &self.id);
            if link.is_some() {
                self.viz = link;
            }
            if let Some(id) = terminal_id(&chunk) {
                append(self.terminal(&id), &data);
            }
        }
        if let Some(status) = exit
            && let Some(id) = terminal_id(&status)
        {
            let exit = Exit {
                code: status.get("exit_code").and_then(Value::as_i64),
                signal: status
                    .get("signal")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            self.terminal(&id).exit = Some(exit.clone());
            self.exit = Some(exit);
        }
        if !meta.is_empty() {
            self.meta.get_or_insert_with(Meta::new).extend(meta);
        }
    }
}

fn terminal_id(value: &Value) -> Option<TerminalId> {
    value
        .get("terminal_id")
        .and_then(Value::as_str)
        .map(|id| TerminalId::new(id.to_owned()))
}

fn append(terminal: &mut Terminal, data: &str) {
    terminal.output.push_str(data);
    let excess = terminal.output.len().saturating_sub(TERMINAL_LIMIT);
    if excess > 0 {
        let start = terminal.output.ceil_char_boundary(excess);
        terminal.output.drain(..start);
        terminal.truncated = true;
    }
}

/// The exit code in a command's raw output: `exit_code` or `exitCode`, at the
/// top or under `details` or `metadata`.
fn exit_code(output: &Value) -> Option<i64> {
    let read = |value: &Value| {
        ["exit_code", "exitCode"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_i64))
    };
    read(output).or_else(|| {
        ["details", "metadata"]
            .iter()
            .find_map(|key| output.get(*key).and_then(read))
    })
}
