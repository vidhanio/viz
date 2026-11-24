//! Transcript history, search, queued inputs, and attachments.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EntryId, Origin, Patch, RunId, ThreadId, ToolCall, TranscriptEntry};

/// A file attached to an input.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attachment {
    /// Display name.
    pub name: String,
    /// Media type.
    pub mime: String,
    /// Base64 bytes, at most 20 MiB decoded.
    pub data: String,
}

impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("name", &self.name)
            .field("mime", &self.mime)
            .finish_non_exhaustive()
    }
}

/// Assigns deterministic Markdown reference labels (`img-N`, `file-N`) in slice
/// order.
///
/// Images receive `img-1`, `img-2`, … and non-image files receive `file-1`,
/// `file-2`, … so the composer, transcript UI, and ACP prompts agree on every
/// label.
#[must_use]
pub fn attachment_ref_labels<'a>(mimes: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut img_idx = 0usize;
    let mut file_idx = 0usize;
    mimes
        .into_iter()
        .map(|mime| {
            if mime.starts_with("image/") {
                img_idx += 1;
                format!("img-{img_idx}")
            } else {
                file_idx += 1;
                format!("file-{file_idx}")
            }
        })
        .collect()
}

/// Formats the canonical inline Markdown reference for an attachment
/// (`![name][img-1]` for images, `[name][file-1]` for non-image files).
#[must_use]
pub fn attachment_inline_md(name: &str, mime: &str, label: &str) -> String {
    let safe_name = name.replace(['[', ']', '\n', '\r'], " ");
    if mime.starts_with("image/") {
        format!("![{safe_name}][{label}]")
    } else {
        format!("[{safe_name}][{label}]")
    }
}

/// Whether Markdown `text` already contains a reference using `label`
/// (case-insensitive per CommonMark link label matching).
#[must_use]
pub fn text_references_label(text: &str, label: &str) -> bool {
    let needle = format!("[{}]", label.to_ascii_lowercase());
    text.to_ascii_lowercase().contains(&needle)
}

/// Attachment capabilities of a thread's runner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct AttachmentCapabilities {
    /// The runner accepts image prompts. Files can always be sent as paths.
    pub images: bool,
}

/// An input waiting for the next turn, removable until dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct QueuedInput {
    pub run: RunId,
    pub text: String,
    /// Number of attachments already stored for this input.
    #[serde(rename = "attachmentCount")]
    pub attachment_count: u64,
}

/// Promote a queued run to the front and stop the active run, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SteerQueuedInput {
    pub thread: ThreadId,
    pub run: RunId,
}

/// Remove an input from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveQueuedInput {
    pub thread: ThreadId,
    pub run: RunId,
}

/// Search across transcripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchThreads {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<u16>,
}

/// An entry that matched a transcript search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct SearchHit {
    pub thread: ThreadId,
    pub entry: EntryId,
    /// Matching text, highlighted ranges delimited by U+0001 and U+0002.
    pub snippet: String,
}

/// Matches for a full-text transcript search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct SearchHits {
    pub hits: Vec<SearchHit>,
}

/// Read entries older than the supplied cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListEntries {
    pub thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<EntryId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// A page of transcript entries, in ascending id order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct EntryPage {
    pub entries: Vec<Entry>,
    pub before: Option<EntryId>,
}

/// Identifies a stored transcript row or an in-progress segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntrySource {
    Stored { entry: EntryId },
    Live { key: String },
}

/// Load an omitted or truncated entry in bounded chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadEntryChunk {
    pub thread: ThreadId,
    pub source: EntrySource,
    /// UTF-8 byte offset, initially zero.
    pub offset: u64,
    /// Use the first chunk's revision for subsequent reads.
    pub revision: Option<String>,
}

/// Concatenate chunks as UTF-8 JSON, then decode as `EntryData` (stored) or
/// `TranscriptEntry` (live).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EntryChunk {
    pub revision: String,
    pub data: String,
    pub next: Option<u64>,
}

/// Reorder a queued run or change its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateQueuedInput {
    pub thread: ThreadId,
    pub run: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub before: Patch<RunId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Entry {
    pub id: EntryId,
    #[serde(flatten)]
    pub data: EntryData,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EntryData {
    Prompt {
        text: String,
        #[serde(default)]
        attachments: Vec<PromptAttachment>,
        /// Files and links the input mentioned.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        context: Vec<PromptContext>,
        origin: Option<Origin>,
        /// Whether `text` was trimmed on the wire to stay within entry byte
        /// budgets while preserving `attachments`, `context`, and `origin`.
        /// Load the complete entry through `thread.entry_chunk`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Transcript(TranscriptEntry),
    /// The full row is available through `thread.entry_chunk`.
    Omitted,
    /// Replaces a tool record at its original transcript position.
    ToolUpdate {
        entry: EntryId,
        record: Box<ToolCall>,
    },
    Run(RunEnd),
    /// The agent continued on its own after its turn ended: the start of an
    /// unprompted run, in place of a prompt.
    Continued,
    TaskResult {
        task: String,
        text: String,
    },
}

/// Something an input points the agent at, sent as an ACP resource link.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptContext {
    /// A file or directory mentioned with `@`; relative to the thread's
    /// directory unless absolute.
    File { path: String },
    /// An issue, pull request, or page mentioned with `#`.
    Link {
        uri: String,
        /// Short label, such as `#42`.
        name: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PromptAttachment {
    pub id: String,
    pub name: String,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunEnd {
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
}

/// The last usage update inside a turn and the tokens reported at its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub context: Option<crate::ContextUsage>,
    pub cost: Option<UsageCost>,
    pub tokens: Option<TurnTokens>,
}

/// The harness's cumulative session cost at the time of the usage update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageCost {
    pub currency: String,
    pub amount: serde_json::Number,
}

/// The harness's prompt response token account. Some harnesses report session
/// totals here, so the fields retain the values exactly as reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnTokens {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
}
