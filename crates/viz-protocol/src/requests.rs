//! Requests for updating threads, projects, worktrees, and queued inputs.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    BlobAttachment, ConfigOption, Harness, OptionValue, Origin, Project, RunId, ThreadId,
    ThreadWorktree, WorktreeTarget,
};

/// A tri-state field patch: omit to keep, `null` to clear, or a value to set.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Patch<T> {
    /// Leave the existing value unchanged (omitted on the wire).
    #[default]
    Keep,
    /// Clear the existing value (`null` on the wire).
    Clear,
    /// Replace the existing value.
    Set(T),
}

impl<T> Patch<T> {
    /// Whether this patch leaves the field unchanged.
    #[must_use]
    pub const fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }

    /// Apply this patch to an optional field.
    #[must_use]
    pub fn apply(self, previous: Option<T>) -> Option<T> {
        match self {
            Self::Keep => previous,
            Self::Clear => None,
            Self::Set(value) => Some(value),
        }
    }
}

impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => serializer.serialize_some(value),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| value.map_or(Self::Clear, Self::Set))
    }
}

impl<T: JsonSchema> JsonSchema for Patch<T> {
    fn schema_name() -> Cow<'static, str> {
        Option::<T>::schema_name()
    }

    fn schema_id() -> Cow<'static, str> {
        Option::<T>::schema_id()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        Option::<T>::json_schema(generator)
    }
}

/// Empty successful response or notification params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Empty {}

/// Rename a thread, or file it under another project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateThread {
    /// The thread to change.
    pub thread: ThreadId,
    /// A new title, if changing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Omit to leave the project alone; `null` clears it.
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub project: Patch<String>,
}

/// Set a thread's configuration option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigureThread {
    /// The thread to configure.
    pub thread: ThreadId,
    /// The option id.
    pub option: String,
    /// The new option value.
    pub value: OptionValue,
}

/// Stop a thread's active turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StopThread {
    /// The thread to stop.
    pub thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
}

/// Delete a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteThread {
    /// The thread to delete.
    pub thread: ThreadId,
    /// Also remove the worktree viz created for the thread, when it is clean
    /// and no other thread uses it. Its branch is kept.
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove_worktree: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Move an idle thread to another checkout of its project's repository. The
/// harness reopens its session there, or starts a new one primed with the
/// transcript when it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveThread {
    /// The thread to move.
    pub thread: ThreadId,
    /// Where to move it.
    pub worktree: WorktreeTarget,
}

/// `thread.move` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Moved {
    /// The new working directory.
    pub cwd: String,
    /// The worktree it now runs in; `None` in the project's checkout.
    pub worktree: Option<ThreadWorktree>,
}

/// List a project's local and remote-tracking branches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListBranches {
    /// Project path.
    pub project: String,
    /// Case-insensitive substring to match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Offset within the filtered branch list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Maximum number of branches to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// `project.branches` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct BranchList {
    /// Short names usable as a worktree base, local branches first.
    pub branches: Vec<GitBranch>,
    pub default: Option<String>,
    pub next: Option<usize>,
}

/// A branch usable as a worktree base.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct GitBranch {
    /// Short name, such as `main` or `origin/main`.
    pub name: String,
    /// Whether this is a remote-tracking branch.
    pub remote: bool,
    /// The currently tracked branch, if configured.
    pub upstream: Option<String>,
    /// A local branch checked out in a different worktree.
    pub checked_out_elsewhere: bool,
}

/// List the worktrees of a project's repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListWorktrees {
    /// Project path.
    pub project: String,
}

/// `worktrees.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct WorktreeList {
    /// The main checkout first, then linked worktrees.
    pub worktrees: Vec<Worktree>,
}

/// One checkout of a repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Worktree {
    /// Absolute path of the worktree root.
    pub path: String,
    /// Checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Whether this is the repository's main checkout.
    pub main: bool,
    /// Whether viz created it, so `worktree.remove` may remove it.
    pub managed: bool,
    /// Threads running in it.
    pub threads: Vec<ThreadId>,
}

/// Remove a clean, unused worktree that viz created. Its branch is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveWorktree {
    /// Absolute path of the worktree root.
    pub path: String,
    /// Discard uncommitted changes and untracked files in it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub force: bool,
}

/// List projects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListProjects {}

/// Projects returned by `projects.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ProjectList {
    /// All projects.
    pub projects: Vec<Project>,
}

/// Add a project directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateProject {
    /// Existing directory; `~` expands to the server's home directory.
    pub path: String,
    /// Display name; omit to use the directory name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Update a project's display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateProject {
    /// Preferences to merge into the project settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<crate::ProjectSettingsPatch>,
    /// Project id.
    pub project: String,
    /// New display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The project returned after creation or an update.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ProjectResult {
    /// The resulting project.
    pub project: Project,
}

/// Remove a project directory from the project list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoveProject {
    /// Project id.
    pub project: String,
}

/// List configured ACP runners.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListHarnesses {}

/// ACP runners returned by `harnesses.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct HarnessList {
    /// Configured ACP runners.
    pub harnesses: Vec<Harness>,
}

/// Fetch the runner's options for a model before creating a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessOptions {
    /// Configured runner name.
    pub harness: String,
    /// Model option value.
    pub model: String,
}

/// The runner's options with the requested model selected.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct HarnessOptionsResult {
    /// Options available for the selected model.
    pub options: Vec<ConfigOption>,
}

/// Search a thread's working directory or a registered project for files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchFiles {
    /// Thread whose working directory is searched. Supply exactly one target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    /// Registered project directory searched for a draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Fuzzy path query.
    pub query: String,
    /// Limit from 1 to 200; omit to use the server default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// Search results from `files.search`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct FileList {
    /// Matches ordered by score, then path.
    pub files: Vec<FileSearchMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct FileSearchMatch {
    /// Path relative to the thread's working directory.
    pub path: String,
    pub kind: FileSearchKind,
    pub score: u32,
    /// Sorted grapheme indices in the relative path, for highlighting.
    pub positions: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileSearchKind {
    File,
    Directory,
}

/// Root directory confining a workspace file read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileRoot {
    /// Confine reads to the thread's working directory.
    Thread {
        /// Owning thread id.
        thread: ThreadId,
    },
    /// Confine reads to a registered project directory.
    Project {
        /// Registered project path.
        project: String,
    },
}

/// Bounded 1-based line window requested from a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LineWindow {
    /// 1-based starting line number.
    pub start: u32,
    /// Maximum number of lines to return.
    pub count: u32,
}

/// Read a file confined to a thread or project root for in-app preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadFile {
    /// Workspace root confining the file read.
    pub root: FileRoot,
    /// Relative path within `root` (no `..` or absolute paths).
    pub path: String,
    /// Optional 1-based line window for text files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<LineWindow>,
}

/// Preview payload returned by `files.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileContent {
    /// Normalized relative path within the requested root.
    pub path: String,
    /// Detected MIME media type.
    pub mime: String,
    /// Total file size in bytes on disk.
    pub size: u64,
    /// Typed preview body.
    pub body: FileBody,
}

/// Typed content payload returned by `files.read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileBody {
    /// UTF-8 text or source code (possibly windowed).
    Text {
        /// Returned UTF-8 text slice.
        text: String,
        /// 1-based line number of the first line in `text`.
        start_line: u32,
        /// Number of lines in `text`.
        line_count: u32,
        /// Total line count when EOF was reached within scan bounds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_lines: Option<u32>,
        /// Whether the returned content was capped by window, byte, or scan
        /// limits.
        #[serde(default)]
        truncated: bool,
        /// Whether additional lines follow this window.
        #[serde(default)]
        has_more: bool,
    },
    /// Raster or vector image whose bytes are fetched via the content
    /// WebSocket (`GetFile`).
    Image {},
    /// Non-text binary file or image exceeding the preview byte limit.
    Binary {
        /// Whether an image exceeded the inline preview size ceiling.
        #[serde(default)]
        too_large: bool,
    },
}

/// Read metadata for one stored attachment belonging to a thread (`blob.stat`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatBlob {
    /// Thread owning the attachment.
    pub thread: ThreadId,
    /// Attachment identifier within the thread.
    pub id: String,
}

/// Metadata for one content-addressed attachment visible to a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BlobInfo {
    /// Attachment identifier within the thread.
    pub id: String,
    /// Lowercase SHA-256 hex digest of the stored content bytes.
    pub blob: String,
    /// Original filename.
    pub name: String,
    /// MIME media type.
    pub mime: String,
    /// Decoded byte length on disk.
    pub size: u64,
}

/// `thread.send` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendInput {
    pub thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<SendMode>,
    /// The input text.
    pub text: String,
    /// Idempotency key: a retry with the same id returns the same run.
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Where the input came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// Content-addressed blobs staged over the content WebSocket to attach to
    /// this input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<BlobAttachment>,
    /// Ids of attachments this thread already stores, attached again, such as
    /// those of a forked prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stored: Vec<String>,
    /// Files and links the input mentions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<crate::PromptContext>,
}

/// How to deliver input to a busy thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SendMode {
    Queue,
    Steer,
}

/// `thread.send` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Sent {
    /// The run the input became.
    pub run: RunId,
}

/// Register this thread's scratch workspace in place and file the threads
/// sharing it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PromoteThread {
    pub thread: ThreadId,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PromotedThread {
    pub project: Project,
    pub threads: Vec<ThreadId>,
}
