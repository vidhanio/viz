//! Requests for updating threads, projects, worktrees, and queued inputs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Attachment, ConfigOption, Harness, OptionValue, Origin, Project, RunId, ThreadId,
    ThreadWorktree, WorktreeTarget,
};

/// Empty successful response or notification params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Empty {}

/// Rename a thread, or file it under another project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UpdateThread<'a> {
    /// The thread to change.
    pub thread: ThreadId,
    /// A new title, if changing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    /// `None` leaves the project alone; `Some(None)` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<Option<&'a str>>,
}

/// Set a thread's configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ConfigureThread<'a> {
    /// The thread to configure.
    pub thread: ThreadId,
    /// The option id.
    pub option: &'a str,
    /// The new option value.
    pub value: &'a OptionValue,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListBranches<'a> {
    /// Project path.
    pub project: &'a str,
    /// Case-insensitive substring to match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<&'a str>,
    /// Offset within the filtered branch list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Maximum number of branches to return.
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListWorktrees<'a> {
    /// Project path.
    pub project: &'a str,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RemoveWorktree<'a> {
    /// Absolute path of the worktree root.
    pub path: &'a str,
    /// Discard uncommitted changes and untracked files in it.
    #[serde(skip_serializing_if = "is_false")]
    pub force: bool,
}

/// List projects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListProjects {}

/// Projects returned by `projects.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ProjectList {
    /// All projects.
    pub projects: Vec<Project>,
}

/// Add a project directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CreateProject<'a> {
    /// Existing directory; `~` expands to the server's home directory.
    pub path: &'a str,
    /// Display name; omit to use the directory name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
}

/// Update a project's display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UpdateProject<'a> {
    /// Preferences to merge into the project settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<crate::ProjectSettingsPatch>,
    /// Project id.
    pub project: &'a str,
    /// New display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
}

/// The project returned after creation or an update.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ProjectResult {
    /// The resulting project.
    pub project: Project,
}

/// Remove a project directory from the project list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RemoveProject<'a> {
    /// Project id.
    pub project: &'a str,
}

/// List configured ACP runners.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
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

/// Search a thread's working directory for files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SearchFiles<'a> {
    /// Thread whose working directory is searched.
    pub thread: ThreadId,
    /// Fuzzy path query.
    pub query: &'a str,
    /// Limit from 1 to 200; omit to use the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    /// Files to attach to this input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
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
