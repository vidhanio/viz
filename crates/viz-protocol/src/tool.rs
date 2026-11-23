//! A tool call as the fold keeps it, and the short text a plain client shows.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ContentBlock, ContentKind, ThreadId};

/// Characters kept of a tool's input when shown as text.
const INPUT_LIMIT: usize = 4000;
/// Characters kept of a tool's output when shown as text.
const OUTPUT_LIMIT: usize = 8000;

const OUTPUT_FIELDS: [&str; 6] = [
    "formatted_output",
    "aggregated_output",
    "output",
    "content",
    "text",
    "result",
];

/// One tool call, as last reported by the harness. Nothing the harness sent is
/// shortened except the output of a terminal it streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// ACP tool call id, or the child session's id for a subagent.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Programmatic tool name, when reported.
    pub name: Option<String>,
    /// The tool's category, when reported.
    pub kind: Option<String>,
    pub status: ToolCallStatus,
    /// Files the call touches, with the line when reported.
    pub locations: Vec<Location>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
    /// What the call produced, in the harness's order.
    pub content: Vec<ToolContent>,
    /// Output of the terminals [`ToolContent::Terminal`] refers to, and of
    /// commands that stream output without showing a terminal.
    pub terminals: Vec<Terminal>,
    /// The call that made this one, when a subagent's calls are reported flat.
    pub parent: Option<String>,
    /// Unix milliseconds when the call was first seen running, or first seen
    /// when the harness does not report that.
    pub started_at: Option<i64>,
    /// Unix milliseconds when the call first completed or failed.
    pub ended_at: Option<i64>,
    /// How a command ended, when reported.
    pub exit: Option<Exit>,
    /// The viz tool the call ran, if it was one.
    pub viz: Option<VizLink>,
    /// Set when the call is an automatic approval review of another call.
    pub review: Option<Review>,
    /// Harness metadata, without the terminal stream.
    pub meta: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

/// Progress of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Not started.
    Pending,
    /// Running.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
}

impl ToolCallStatus {
    /// Whether the call has ended.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// A place in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// One block of a tool call's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum ToolContent {
    /// Text, an image, audio, a resource link, or an embedded resource.
    Block(Box<ContentBlock>),
    Diff(ToolDiff),
    /// A terminal, described in [`ToolCall::terminals`].
    Terminal(String),
    /// A child session the call operates on.
    Session(String),
}

/// A file's contents before and after an edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiff {
    pub path: String,
    /// Previous content; `None` for a new file.
    pub old_text: Option<String>,
    pub new_text: String,
}

/// A command's output so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Terminal {
    pub id: String,
    pub output: String,
    /// Whether the start of the output was dropped.
    pub truncated: bool,
    pub exit: Option<Exit>,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Exit {
    pub code: Option<i64>,
    pub signal: Option<String>,
}

/// The viz tool behind a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VizLink {
    pub tool: String,
    /// The thread the call started or addressed.
    pub child: Option<ThreadId>,
    pub environment: Option<String>,
}

impl ToolCall {
    /// A call of which only the id is known.
    #[must_use]
    pub fn new(id: String) -> Self {
        Self {
            title: id.clone(),
            id,
            name: None,
            kind: None,
            status: ToolCallStatus::Pending,
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
            content: Vec::new(),
            terminals: Vec::new(),
            parent: None,
            started_at: None,
            ended_at: None,
            exit: None,
            viz: None,
            review: None,
            meta: None,
            more: false,
        }
    }

    /// The input as short text. Structured input stays JSON, so clients can
    /// read fields such as `command` or `path`.
    #[must_use]
    pub fn input_text(&self) -> Option<String> {
        let text = match self.raw_input.as_ref()? {
            Value::String(text) => truncate(text, INPUT_LIMIT),
            value => truncate(&pretty(value), INPUT_LIMIT),
        };
        (!text.is_empty()).then_some(text)
    }

    /// The output as short text: text content, else terminal output, else
    /// readable raw output.
    #[must_use]
    pub fn output_text(&self) -> Option<String> {
        let text = join(self.content.iter().filter_map(|content| match content {
            ToolContent::Block(block) if block.kind == ContentKind::Text => {
                block.payload.get("text").and_then(Value::as_str)
            }
            _ => None,
        }));
        let text = if text.is_empty() {
            join(
                self.terminals
                    .iter()
                    .map(|terminal| terminal.output.as_str()),
            )
        } else {
            text
        };
        let text = if text.is_empty() {
            self.raw_output
                .as_ref()
                .map_or_default(|output| raw_text(output, 0).unwrap_or_else(|| pretty(output)))
        } else {
            text
        };
        let mut text = text.trim();
        if self.kind.as_deref() == Some("execute")
            && self.title.starts_with("$ ")
            && let Some(rest) = text.strip_prefix(self.title.as_str())
            && (rest.is_empty() || rest.starts_with('\n'))
        {
            text = rest.trim_start_matches('\n');
        }
        (!text.is_empty()).then(|| truncate(text, OUTPUT_LIMIT))
    }

    /// The file changes among the content.
    pub fn diffs(&self) -> impl Iterator<Item = &ToolDiff> {
        self.content.iter().filter_map(|content| match content {
            ToolContent::Diff(diff) => Some(diff),
            _ => None,
        })
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let parts: Vec<_> = parts.filter(|part| !part.is_empty()).collect();
    parts.join("\n")
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

/// The verdict of a harness's own reviewer on another call.
///
/// codex-acp reports one `think` call titled `Guardian Review` next to every
/// command it assesses. The fold gives it the reviewed action as its title,
/// and a client can fold the record into the call it reviews or hide it unless
/// it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// The reviewed call, when the harness names it.
    pub target: Option<String>,
    /// What was reviewed, such as the command.
    pub action: Option<String>,
    /// The harness's word for the verdict, such as `approved` or `denied`.
    pub verdict: Option<String>,
    pub risk: Option<String>,
    pub authorization: Option<String>,
    pub rationale: Option<String>,
}

impl ToolCall {
    /// Whether this is a review with nothing to tell: pending or approved.
    #[must_use]
    pub fn is_routine_review(&self) -> bool {
        self.review.is_some() && self.status != ToolCallStatus::Failed
    }
}
