//! What a turn produced: the entries a caller stores and renders.
//!
//! Every [`Entry`] has a key that stays the same from its first live
//! appearance to its commit, so a client can animate and diff it. Keys are
//! `<turn>:<n>`, where `n` counts up in the order things started. A long tool
//! call can be committed after text that started later; [`Entry::order`]
//! recovers the start order.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{schema::ContentBlock, tool::ToolCall};

/// One part of a turn's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Stable identity, shared by the live and the committed entry.
    pub key: String,
    /// Where the entry started relative to the others of its turn.
    pub order: u32,
    pub body: Body,
}

/// The content of an [`Entry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// A run of streamed text. Long messages are committed as several segments, so
/// little is lost when the server stops mid-turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub text: String,
    /// Key of the message's first segment. Adjacent segments of one message
    /// are one text, without a separator.
    pub message: String,
    /// Unix milliseconds of the first and last observed chunk.
    pub started_at: i64,
    pub ended_at: i64,
}

/// An image, audio clip, or resource in the agent's own output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub block: ContentBlock,
    pub source: TextKind,
    /// Unix milliseconds when the block was observed.
    pub at: i64,
}

/// Whether streamed content is prose or thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextKind {
    Text,
    Thought,
}

/// A plan as it stood at one moment; each change is a new entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// The harness's plan id, when it keeps several plans.
    pub id: Option<String>,
    pub content: PlanContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PlanContent {
    Items { entries: Vec<PlanEntry> },
    Markdown { text: String, path: Option<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// The running turn's uncommitted entries, in start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTurn {
    /// The caller's identity for the turn, the prefix of its entry keys.
    pub turn: String,
    pub entries: Vec<Entry>,
}
