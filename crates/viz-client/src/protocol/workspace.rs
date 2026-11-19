//! Workspace history, attachments, and Git requests.

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

/// Create a worktree on a new branch when creating a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Worktree {
    /// New branch name.
    pub branch: String,
    /// Starting ref; absent uses the project's current branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
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

/// Snapshot of the repository containing the conversation's working directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitState {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    /// The viz-created worktree's path, if any.
    pub worktree: Option<String>,
    pub files: Vec<GitFile>,
}

/// Working tree status for a path relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GitFile {
    pub path: String,
    pub status: GitFileStatus,
    pub staged: bool,
}

/// Status as reported by the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

/// Comparison base for a diff request.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffScope {
    /// Uncommitted changes.
    Worktree,
    /// Changes since the start of a turn.
    Turn,
    /// Changes against the base branch.
    Branch,
}

/// Read a diff of the requested scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitDiff<'a> {
    pub conversation: ConversationId,
    pub scope: DiffScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<EntryId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<&'a str>,
}

/// Status of a file as represented in a diff.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffFileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

/// One path and its unified patch, capped at 200 KiB.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: DiffFileStatus,
    pub additions: i64,
    pub deletions: i64,
    pub patch: String,
}

/// Files that differ in the requested scope.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiffFiles {
    pub files: Vec<DiffFile>,
}

/// List available branches in the conversation's repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitBranches {
    pub conversation: ConversationId,
}

/// Available branches; `current` is absent when detached.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Branches {
    pub current: Option<String>,
    pub branches: Vec<String>,
}

/// Check out an existing branch or create a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitCheckout<'a> {
    pub conversation: ConversationId,
    pub branch: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<bool>,
}

/// Commit changes; an omitted or empty message is generated by the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitCommit<'a> {
    pub conversation: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'a str>,
    /// Stage all changes first; defaults to true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
}

/// A newly created Git commit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommitResult {
    pub commit: String,
    pub message: String,
}

/// Push the conversation's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitPush {
    pub conversation: ConversationId,
}

/// Create a pull request for the conversation's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitPr<'a> {
    pub conversation: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
}

/// Location of a newly created pull request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PrResult {
    pub url: String,
}

/// Restore the working tree as it was before the chosen prompt entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitRevert {
    pub conversation: ConversationId,
    pub turn: EntryId,
}
