//! Transcript history, search, queued inputs, and attachments.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Origin, RunId, ThreadId, ToolRecord};

/// A transcript entry id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct EntryId(pub u64);

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

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
}

/// Remove an input from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveQueuedInput {
    pub thread: ThreadId,
    pub run: RunId,
}

/// Search across transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SearchThreads<'a> {
    pub query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
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

/// Reorder a queued run or change its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UpdateQueuedInput<'a> {
    pub thread: ThreadId,
    pub run: RunId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Option<RunId>>,
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
        origin: Option<Origin>,
    },
    Text {
        text: String,
        key: String,
    },
    Thought {
        text: String,
        key: String,
    },
    Tool(ToolRecord),
    Plan {
        entries: Vec<PlanEntry>,
    },
    Run(RunEnd),
    TaskResult {
        task: String,
        text: String,
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
}
