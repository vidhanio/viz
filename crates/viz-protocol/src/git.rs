//! Checkout state, mutations and structured patches.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{RunId, ThreadId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStatusRequest {
    pub thread: ThreadId,
}

/// Subscribe to checkout status on the current WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WatchGit {
    pub thread: ThreadId,
}

/// Release a checkout status subscription on the current WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnwatchGit {
    pub thread: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStatus {
    pub checkout: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicted: usize,
    pub stats: GitLineStats,
    pub operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitActionRequest {
    pub thread: ThreadId,
    pub action: GitAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitAction {
    CreateBranch { name: String, start: String },
    SwitchBranch { name: String },
    Commit { paths: Vec<String>, message: String },
    Push { remote: String },
    Fetch { remote: String },
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitActionResult {
    /// The new commit ID for a commit action.
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitFailure {
    Busy,
    Dirty,
    Conflicted,
    Cancelled,
    NotRepository,
    Invalid,
    StaleCheckpoint,
    CheckpointObstruction,
    NonFastForward,
    Authentication,
    HostKey,
    HookRejected,
    NothingToCommit,
    MissingUpstream,
    MissingIdentity,
    BranchAlreadyExists,
    Command,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffRequest {
    pub thread: ThreadId,
    pub target: GitDiffTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitDiffTarget {
    Working,
    Staged,
    Base { revision: String },
    Commit { revision: String },
    Turn { run: RunId },
    Thread,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitLineStats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiff {
    pub revision: String,
    pub stats: GitLineStats,
    pub truncated: bool,
    pub files: Vec<GitDiffFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub kind: GitFileKind,
    pub binary: bool,
    pub truncated: bool,
    pub stats: Option<GitLineStats>,
    pub hunks: Vec<GitHunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitFileKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    Conflicted,
    Untracked,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<GitDiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffLine {
    pub origin: char,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: String,
}

/// The worktree state at the start or end of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointEdge {
    Before,
    After,
}

/// Restore worktree files only if they still match the end of the expected run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreCheckpoint {
    pub thread: ThreadId,
    pub run: RunId,
    pub edge: CheckpointEdge,
    pub expected_run: RunId,
}
