//! Continue a copy of a thread from one of its points.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EntryId, PromptAttachment, ThreadId, WorktreeTarget};

/// Copy a thread's history into a new idle thread. A projectless fork gets its
/// own scratch folder. A project fork shares the source checkout unless a
/// different worktree is requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForkThread {
    pub thread: ThreadId,
    #[serde(default)]
    pub point: ForkPoint,
    /// Another checkout of the project to run in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeTarget>,
    /// Another harness to continue with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Defaults to the source's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Idempotency key: a retry with the same id returns the same thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Where a fork leaves its source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "at", rename_all = "lowercase")]
pub enum ForkPoint {
    /// After its last finished run.
    #[default]
    Head,
    /// After the run containing this entry.
    Through { entry: EntryId },
    /// Before this prompt, to send another version of it.
    Before { entry: EntryId },
}

/// `thread.fork` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Forked {
    pub thread: ThreadId,
    /// The prompt the fork was made before, to edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<ForkedPrompt>,
}

/// The prompt a fork leaves out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkedPrompt {
    pub text: String,
    /// Stored in the source thread.
    pub attachments: Vec<PromptAttachment>,
}

/// Where a thread was forked from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForkSource {
    /// The source thread, which may be gone.
    pub thread: ThreadId,
    /// The last of its entries the fork shares, if any.
    pub entry: Option<EntryId>,
}
