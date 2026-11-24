//! What a turn produced: the entries a caller stores and renders.
//!
//! Every [`TranscriptEntry`] has a key that stays the same from its first live
//! appearance to its commit, so a client can animate and diff it. Keys are
//! `<turn>:<n>`, where `n` counts up in the order things started. A long tool
//! call can be committed after text that started later;
//! [`TranscriptEntry::order`] recovers the start order.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PlanEntry, ToolCall};

/// One part of a turn's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    /// Stable identity, shared by the live and the committed entry.
    pub key: String,
    /// Where the entry started relative to the others of its turn.
    pub order: u64,
    pub body: Body,
}

/// The content of an [`TranscriptEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum Body {
    /// Agent prose.
    Text(Segment),
    /// Agent thinking.
    Thought(Segment),
    /// A non-text block the agent sent as part of a message or a thought.
    Media(Box<Media>),
    Tool(Box<ToolCall>),
    Plan(Plan),
    /// The harness summarised the conversation to free its context.
    Compaction(Compaction),
}

/// A context compaction, live while it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Compaction {
    /// The harness's id, unique within the session.
    pub id: String,
    pub status: CompactionStatus,
    /// What the harness kept, for the user to read.
    pub summary: String,
    pub error: Option<String>,
    /// Context tokens used before compaction, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    /// Context tokens used after compaction, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    /// Unix milliseconds of the first and last report.
    pub started_at: i64,
    pub ended_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// A run of streamed text. Long messages are committed as several segments, so
/// little is lost when the server stops mid-turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub text: String,
    /// Key of the message's first segment. Adjacent segments of one message
    /// are one text, without a separator.
    pub message: String,
    /// Unix milliseconds of the first and last observed chunk.
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// On-disk content-addressed media payload extracted at server ingestion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaBlob {
    /// Thread-scoped attachment identifier (canonical SHA-256 hex digest)
    /// readable via the content WebSocket (`GetAttachment`).
    pub id: String,
    /// Media MIME type.
    pub mime: String,
    /// Decoded byte length stored on disk.
    pub size: u64,
}

/// An image, audio clip, or resource in the agent's own output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub block: ContentBlock,
    /// Content-addressed blob reference when inline binary bytes were extracted
    /// to disk at ingestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<MediaBlob>,
    pub source: TextKind,
    /// Unix milliseconds when the block was observed.
    pub at: i64,
}

/// Whether streamed content is prose or thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextKind {
    Text,
    Thought,
}

/// A plan as it stood at one moment; each change is a new entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The harness's plan id, when it keeps several plans.
    pub id: Option<String>,
    /// Unix milliseconds when observed, when known.
    pub at: Option<i64>,
    pub content: PlanContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PlanContent {
    Items { entries: Vec<PlanEntry> },
    Markdown { text: String, path: Option<PathBuf> },
}

/// The running turn's uncommitted entries, in start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiveTurn {
    /// The caller's identity for the turn, the prefix of its entry keys.
    pub turn: String,
    pub entries: Vec<TranscriptEntry>,
}

/// One ACP content block, including the payload, annotations and extension
/// fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContentBlock {
    pub kind: ContentKind,
    /// The original block with its ACP field names.
    pub payload: serde_json::Value,
}

/// The content types clients can use to render a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Image,
    Audio,
    ResourceLink,
    Resource,
    Other,
}

/// Changes to a previously delivered live snapshot. Entries replace values
/// with matching keys. A changed turn discards the previous tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LiveDelta {
    pub turn: Option<String>,
    pub upsert: Vec<TranscriptEntry>,
    pub remove: Vec<String>,
}

impl LiveDelta {
    /// Compare bounded snapshots of two successive views of a thread.
    #[must_use]
    pub fn between(prior: Option<&LiveTurn>, next: Option<&LiveTurn>) -> Self {
        let Some(next) = next else {
            return Self {
                turn: None,
                upsert: Vec::new(),
                remove: Vec::new(),
            };
        };
        let prior = prior.filter(|prior| prior.turn == next.turn);
        let previous = prior
            .into_iter()
            .flat_map(|prior| &prior.entries)
            .map(|entry| (entry.key.as_str(), entry))
            .collect::<std::collections::HashMap<_, _>>();
        let next_keys = next
            .entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<std::collections::HashSet<_>>();
        Self {
            turn: Some(next.turn.clone()),
            upsert: next
                .entries
                .iter()
                .filter(|entry| previous.get(entry.key.as_str()).copied() != Some(*entry))
                .cloned()
                .collect(),
            remove: prior
                .into_iter()
                .flat_map(|prior| &prior.entries)
                .filter(|entry| !next_keys.contains(entry.key.as_str()))
                .map(|entry| entry.key.clone())
                .collect(),
        }
    }

    /// Apply to the client's live snapshot after applying preceding changes.
    pub fn apply(self, live: &mut Option<LiveTurn>) {
        let Some(turn) = self.turn else {
            *live = None;
            return;
        };
        let mut entries = live
            .take()
            .filter(|prior| prior.turn == turn)
            .map_or_default(|prior| prior.entries);
        let replacing: std::collections::HashSet<_> =
            self.upsert.iter().map(|entry| entry.key.as_str()).collect();
        let removing: std::collections::HashSet<_> =
            self.remove.iter().map(String::as_str).collect();
        entries.retain(|entry| {
            !replacing.contains(entry.key.as_str()) && !removing.contains(entry.key.as_str())
        });
        entries.extend(self.upsert);
        entries.sort_unstable_by_key(|entry| entry.order);
        *live = Some(LiveTurn { turn, entries });
    }
}
