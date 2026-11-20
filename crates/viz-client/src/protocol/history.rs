//! Transcript history, search, queued inputs, and attachments.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ConversationId, SubmissionId};

/// A pi-durable transcript entry id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(pub u64);

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A file attached to an input.
#[derive(Clone, PartialEq, Eq, Serialize)]
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

/// Attachment capabilities of a conversation's runner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct AttachmentCapabilities {
    /// The runner accepts image prompts. Files can always be sent as paths.
    pub images: bool,
}

/// An input waiting for the next turn, removable until dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueuedInput {
    pub submission: SubmissionId,
    pub text: String,
}

/// Remove an input from the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RemoveQueuedInput {
    pub conversation: ConversationId,
    pub submission: SubmissionId,
}

/// Search across transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SearchConversations<'a> {
    pub query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// An entry that matched a transcript search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHit {
    pub conversation: ConversationId,
    pub entry: EntryId,
    /// Matching text, highlighted ranges delimited by U+0001 and U+0002.
    pub snippet: String,
}

/// Matches for a full-text transcript search.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SearchHits {
    pub hits: Vec<SearchHit>,
}

/// Read entries older than the supplied cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ListEntries {
    pub conversation: ConversationId,
    pub before: EntryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// A page of raw transcript entries, in ascending id order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EntryPage {
    pub entries: Vec<Value>,
    pub before: Option<EntryId>,
}
