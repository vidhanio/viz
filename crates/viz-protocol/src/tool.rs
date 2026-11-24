//! A tool call as the fold keeps it, and the short text a plain client shows.

use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ContentBlock, ContentKind, ThreadId};

/// Characters kept of a tool's input when shown as text.
const INPUT_LIMIT: usize = 4000;
/// Characters kept of a tool's output when shown as text.
const OUTPUT_LIMIT: usize = 8000;

/// One tool call, as last reported by the harness. Nothing the harness sent is
/// shortened except the output of a terminal it streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// Tool-specific fields for clients.
    pub payload: ToolPayload,
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
    pub locations: Vec<FileTarget>,
    /// Formatted structured arguments for inspection.
    pub arguments: Option<String>,
    /// Display text derived from raw output or a harness envelope.
    pub display_output: Option<String>,
    /// Task id confirmed by a delegation receipt.
    pub task_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, Value>>,
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

/// A file and its optional one-based inclusive line range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileTarget {
    pub path: PathBuf,
    #[serde(flatten)]
    pub location: Location,
}

/// One-based inclusive bounds. A single-line location has equal bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub line: Option<u32>,
    pub end_line: Option<u32>,
}

/// Semantic tool input and structured search output as reported by the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum ToolPayload {
    Execute {
        command: Option<String>,
        timeout_ms: Option<u64>,
    },
    Read {
        targets: Vec<FileTarget>,
    },
    Edit {
        operation: FileOperation,
        targets: Vec<FileTarget>,
    },
    Search {
        query: Option<String>,
        matches: Vec<SearchMatch>,
    },
    Fetch {
        url: Option<String>,
        matches: Vec<SearchMatch>,
    },
    Mcp {
        summary: Option<String>,
    },
    Other {
        summary: Option<String>,
    },
}

/// Which filesystem operation an edit performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FileOperation {
    Write,
    Edit,
    Delete,
    Move,
}

/// One reported match in a local file or on the web.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SearchMatch {
    File { target: FileTarget, snippet: String },
    Web { url: String, snippet: String },
}

impl ToolPayload {
    /// File targets in harness order, or the requested path when inferred from
    /// arguments.
    #[must_use]
    pub fn targets(&self) -> &[FileTarget] {
        match self {
            Self::Read { targets } | Self::Edit { targets, .. } => targets,
            _ => &[],
        }
    }
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

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if requires &T"
)]
const fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// A file's contents before and after an edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiff {
    pub path: String,
    /// Previous content; `None` for a new file.
    pub old_text: Option<String>,
    pub new_text: String,
    /// Leading unchanged lines stripped from both `old_text` and `new_text`
    /// during wire projection; clients add `skip` to computed hunk start lines.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skip: u32,
    /// Added lines in the full edit.
    pub added: u32,
    /// Removed lines in the full edit.
    pub removed: u32,
}

impl ToolDiff {
    /// Constructs an untrimmed edit and computes the counts before projection.
    #[must_use]
    pub fn new(path: String, old_text: Option<String>, new_text: String) -> Self {
        let (added, removed) = line_stats(old_text.as_deref(), &new_text);
        Self {
            path,
            old_text,
            new_text,
            skip: 0,
            added,
            removed,
        }
    }
}

/// Counts changed lines. Large changed regions use a multiset estimate to
/// bound the cost of comparing reordered lines.
fn line_stats(old_text: Option<&str>, new_text: &str) -> (u32, u32) {
    let new_lines: Vec<&str> = new_text.lines().collect();
    let Some(old_text) = old_text else {
        return (u32::try_from(new_lines.len()).unwrap_or(u32::MAX), 0);
    };
    let old_lines: Vec<&str> = old_text.lines().collect();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "comparing trailing lines at independent old_end and new_end indices"
    )]
    while old_end > prefix && new_end > prefix && old_lines[old_end - 1] == new_lines[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let a = &old_lines[prefix..old_end];
    let b = &new_lines[prefix..new_end];
    if a.is_empty() || b.is_empty() {
        return (
            u32::try_from(b.len()).unwrap_or(u32::MAX),
            u32::try_from(a.len()).unwrap_or(u32::MAX),
        );
    }
    let common = if a.len().saturating_mul(b.len()) <= 65_536 {
        lcs_len(a, b)
    } else {
        multiset_common(a, b)
    };
    let added = b.len().saturating_sub(common);
    let removed = a.len().saturating_sub(common);
    (
        u32::try_from(added).unwrap_or(u32::MAX),
        u32::try_from(removed).unwrap_or(u32::MAX),
    )
}

fn lcs_len(a: &[&str], b: &[&str]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];
    for &x in a {
        for (j, &y) in b.iter().enumerate() {
            curr[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(curr[j])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    prev[b.len()]
}

fn multiset_common<'a>(a: &[&'a str], b: &[&'a str]) -> usize {
    let mut counts: BTreeMap<&'a str, usize> = BTreeMap::new();
    for &line in a {
        *counts.entry(line).or_insert(0) += 1;
    }
    let mut common = 0;
    for &line in b {
        if let Some(slot) = counts.get_mut(line)
            && *slot > 0
        {
            *slot -= 1;
            common += 1;
        }
    }
    common
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
            payload: ToolPayload::Other { summary: None },
            arguments: None,
            display_output: None,
            task_id: None,
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
        }
    }

    /// The original input as short text for generic inspection.
    #[must_use]
    pub fn input_text(&self) -> Option<String> {
        let text = match self.raw_input.as_ref()? {
            Value::String(text) => truncate(text, INPUT_LIMIT),
            value => truncate(&pretty(value), INPUT_LIMIT),
        };
        (!text.is_empty()).then_some(text)
    }

    /// The normalized output as short text for inline previews.
    #[must_use]
    pub fn output_text(&self) -> Option<String> {
        self.render_output(Some(OUTPUT_LIMIT))
    }

    /// Renders a complete output downloaded over the content plane.
    #[must_use]
    pub fn full_output_text(&self) -> Option<String> {
        self.render_output(None)
    }

    /// Renders source content for ACP normalization before any display
    /// override.
    #[must_use]
    pub fn structured_output_text(&self) -> Option<String> {
        self.render_source_output(None)
    }

    fn render_output(&self, limit: Option<usize>) -> Option<String> {
        if let Some(text) = &self.display_output {
            return (!text.is_empty())
                .then(|| limit.map_or_else(|| text.clone(), |max| truncate(text, max)));
        }
        self.render_source_output(limit)
    }

    fn render_source_output(&self, limit: Option<usize>) -> Option<String> {
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
        let text = self.trim_output(&text);
        (!text.is_empty()).then(|| limit.map_or_else(|| text.to_owned(), |max| truncate(text, max)))
    }

    /// Strips outer whitespace and a harness's echoed command title.
    #[must_use]
    pub fn trim_output<'a>(&self, text: &'a str) -> &'a str {
        let text = text.trim();
        if matches!(self.payload, ToolPayload::Execute { .. })
            && self.title.starts_with("$ ")
            && let Some(rest) = text.strip_prefix(self.title.as_str())
            && (rest.is_empty() || rest.starts_with('\n'))
        {
            return rest.trim_start_matches('\n');
        }
        text
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
