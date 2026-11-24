//! Shared protocol v2 types for viz servers and clients.
//!
//! Each request's params type implements [`Method`], which names the method on
//! the wire and its result type.

mod catalog;
mod connections;
mod folders;
mod fork;
mod form;
mod git;
mod github;
mod history;
mod ids;
mod interactions;
mod lifecycle;
pub mod limits;
pub mod peer;
mod project_settings;
mod requests;
mod threads;
mod tool;
mod transcript;

pub use catalog::{
    ContextUsage, Harness, HarnessSession, HarnessSessions, HarnessStatus, ImportSession,
    ListHarnessSessions, Project, ThreadWorktree, WorktreeTarget,
};
pub use connections::{
    CancelTask, ClearTasks, Cleared, DelegateInput, DelegateTask, DeleteTask, Environment,
    EnvironmentList, EnvironmentStatus, ListEnvironments, ListTasks, Notification, ReadProject,
    ReadThreadState, Run, RunKind, RunStatus, Task, TaskList, TaskResult, TaskStatus,
    TaskStatusRequest, TaskStatusResult, ThreadStateResult, Unwatch, WaitResult, WaitThread, Watch,
    Watching,
};
pub use folders::{
    BrowseFolders, CancelClone, CloneId, ClonePhase, CloneState, CloneStatus, CreateFolder,
    FolderCreated, FolderEntries, FolderEntry, FolderIssue, GitHubRepositories, GitHubRepository,
    GitHubRepositoryList, InitRepository, ReadClone, StartClone,
};
pub use fork::{ForkPoint, ForkSource, ForkThread, Forked, ForkedPrompt};
pub use form::{FieldKind, QuestionField, question_fields};
pub use git::{
    CheckpointEdge, GitAction, GitActionRequest, GitActionResult, GitDiff, GitDiffFile,
    GitDiffLine, GitDiffRequest, GitDiffTarget, GitFailure, GitFileKind, GitHunk, GitLineStats,
    GitStatus, GitStatusRequest, RestoreCheckpoint, UnwatchGit, WatchGit,
};
pub use github::{
    GitHubAuth, GitHubAuthState, GitHubCheck, GitHubChecks, GitHubCreatePull, GitHubItem,
    GitHubItemKind, GitHubLaunch, GitHubPull, GitHubPullPage, GitHubPullState, GitHubReview,
    GitHubSearch, GitHubSearchResult,
};
pub use history::{
    Attachment, AttachmentCapabilities, Entry, EntryChunk, EntryData, EntryPage, EntrySource,
    ListEntries, PlanEntry, PlanPriority, PlanStatus, PromptAttachment, PromptContext, QueuedInput,
    ReadEntryChunk, RemoveQueuedInput, RunEnd, SearchHit, SearchHits, SearchThreads,
    SteerQueuedInput, TurnTokens, TurnUsage, UpdateQueuedInput, UsageCost, attachment_inline_md,
    attachment_ref_labels, text_references_label,
};
pub use ids::{EntryId, RequestId, RunId, TaskId, ThreadId};
pub use interactions::{
    Access, AccessKind, AccessMode, AccessSource, AnswerRequest, AnsweredBy, FormValue,
    NativeAccess, PermissionKind, PermissionOption, RequestAnswer, RequestBody, RequestState,
    RunRequest, SetProjectAccess, SetThreadAccess, Withdrawal,
};
pub use lifecycle::{
    Attention, AutoSettle, Lifecycle, Position, RetitleThread, SettledBy, Shelf, Standing,
    ThreadUpdated, TriageAction, TriageThread, Triaged,
};
pub use project_settings::{
    DefaultCheckout, ProjectAutoSettle, ProjectIcon, ProjectIconChange, ProjectIconResult,
    ProjectSettings, ProjectSettingsPatch, ReadProjectIcon, Setting,
};
pub use requests::{
    BranchList, ConfigureThread, CreateProject, DeleteThread, Empty, FileContent, FileList,
    FileSearchKind, FileSearchMatch, GitBranch, HarnessList, HarnessOptions, HarnessOptionsResult,
    ListBranches, ListHarnesses, ListProjects, ListWorktrees, MoveThread, Moved, Patch,
    ProjectList, ProjectResult, PromoteThread, PromotedThread, ReadAttachment, ReadFile,
    RemoveProject, RemoveWorktree, SearchFiles, SendInput, SendMode, Sent, StopThread,
    UpdateProject, UpdateThread, Worktree, WorktreeList,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
pub use threads::{
    CommandSource, ConfigOption, DependentChoice, DependentOption, LaunchThread, Launched,
    ListThreads, OptionChoice, OptionKind, OptionValue, SlashCommand, ThreadList, ThreadState,
    ThreadStatus, ThreadSummary,
};
pub use tool::{
    Exit, Location, Review, Terminal, ToolCall, ToolCallStatus, ToolContent, ToolDiff, VizLink,
};
pub use transcript::{
    Body, Compaction, CompactionStatus, ContentBlock, ContentKind, LiveDelta, LiveTurn, Media,
    Plan, PlanContent, Segment, TextKind, TranscriptEntry,
};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// A request method: its params type, its name on the wire, and its result
/// type. Sealed: the protocol defines every method.
pub trait Method: Serialize + DeserializeOwned + Send + Sync + private::Sealed {
    /// The JSON-RPC method name.
    const NAME: &'static str;
    /// The `result` of a successful call.
    type Response: Clone + DeserializeOwned + Serialize;
}

mod private {
    pub trait Sealed {}
}

macro_rules! methods {
    ($($params:ty => $name:literal, $response:ty;)*) => {
        $(
        impl private::Sealed for $params {}
        impl Method for $params {
            const NAME: &'static str = $name;
            type Response = $response;
        }
        )*

        /// Input and output schemas for every public wire method.
        #[must_use]
        pub fn method_schemas() -> serde_json::Value {
            serde_json::Value::Object([
                $(($name.to_owned(), serde_json::json!({
                    "params": schemars::schema_for!($params),
                    "result": schemars::schema_for!($response),
                })),)*
            ].into_iter().collect())
        }
    };
}

methods! {
    HelloParams => "hello", Hello;
    ListEnvironments => "environments.list", EnvironmentList;
    ListTasks => "tasks.list", TaskList;
    TaskStatusRequest => "task.status", TaskStatusResult;
    CancelTask => "task.cancel", TaskResult;
    DelegateTask => "task.delegate", TaskResult;
    DeleteTask => "task.delete", Empty;
    ClearTasks => "tasks.clear", Cleared;
    WaitThread => "thread.wait", WaitResult;
    ReadThreadState => "thread.state", ThreadStateResult;
    ReadProject => "project.read", ProjectResult;
    SteerQueuedInput => "queue.steer", Empty;
    UpdateQueuedInput => "queue.update", Empty;
    ListThreads => "threads.list", ThreadList;
    UpdateThread => "thread.update", ThreadUpdated;
    RetitleThread => "thread.retitle", ThreadUpdated;
    TriageThread => "thread.triage", Triaged;
    ConfigureThread => "thread.configure", Empty;
    StopThread => "thread.stop", Empty;
    DeleteThread => "thread.delete", Empty;
    ListProjects => "projects.list", ProjectList;
    CreateProject => "project.create", ProjectResult;
    UpdateProject => "project.update", Empty;
    ReadProjectIcon => "project.icon", ProjectIconResult;
    BrowseFolders => "folders.browse", FolderEntries;
    CreateFolder => "folder.create", FolderCreated;
    GitStatusRequest => "git.status", GitStatus;
    GitActionRequest => "git.action", GitActionResult;
    RestoreCheckpoint => "git.restoreCheckpoint", Empty;
    GitDiffRequest => "git.diff", GitDiff;
    WatchGit => "git.watch", GitStatus;
    UnwatchGit => "git.unwatch", Empty;
    InitRepository => "git.init", FolderCreated;
    StartClone => "project.clone", CloneStatus;
    ReadClone => "project.cloneStatus", CloneStatus;
    CancelClone => "project.cancelClone", CloneStatus;
    GitHubAuth => "github.auth", GitHubAuthState;
    GitHubSearch => "github.search", GitHubSearchResult;
    GitHubChecks => "github.checks", Vec<GitHubCheck>;
    GitHubPullPage => "github.reviews", Vec<GitHubReview>;
    GitHubCreatePull => "github.createPull", GitHubItem;
    GitHubLaunch => "github.launch", Launched;
    GitHubRepositories => "github.repositories", GitHubRepositoryList;
    RemoveProject => "project.remove", Empty;
    ListHarnesses => "harnesses.list", HarnessList;
    HarnessOptions => "harnesses.options", HarnessOptionsResult;
    ListHarnessSessions => "harness.sessions", HarnessSessions;
    ImportSession => "thread.import", Launched;
    SearchFiles => "files.search", FileList;
    ReadFile => "files.read", FileContent;
    ReadAttachment => "thread.attachment", Attachment;
    LaunchThread => "thread.launch", Launched;
    ForkThread => "thread.fork", Forked;
    SendInput => "thread.send", Sent;
    Watch => "thread.watch", Watching;
    Unwatch => "thread.unwatch", Watching;
    SearchThreads => "threads.search", SearchHits;
    ListEntries => "thread.entries", EntryPage;
    ReadEntryChunk => "thread.entry_chunk", EntryChunk;
    AnswerRequest => "thread.answer", RunRequest;
    SetThreadAccess => "thread.access", Empty;
    SetProjectAccess => "project.access", Empty;
    RemoveQueuedInput => "queue.remove", Empty;
    ListBranches => "project.branches", BranchList;
    ListWorktrees => "worktrees.list", WorktreeList;
    RemoveWorktree => "worktree.remove", Empty;
    MoveThread => "thread.move", Moved;
    PromoteThread => "thread.promoteProject", PromotedThread;
}

/// What kind of client is connecting; decides which notifications it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    /// Full UI (desktop, mobile): watches whole threads.
    App,
    /// Connector (Discord, iMessage, ...): receives replies to its own inputs
    /// only.
    Connector,
    /// One-off command-line client.
    Cli,
}

/// Identifies a client in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    /// Client kind.
    pub kind: ClientKind,
    /// Client name; for connectors, the name inputs' [`Origin::connector`] use.
    pub name: String,
}

/// Where an input came from, so its reply is routed back there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Origin {
    /// Connector name, matching its `hello` name.
    pub connector: String,
    /// Connector-specific channel/chat id the reply goes to.
    pub channel: String,
    /// Connector-specific thread id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// Connector-specific id of the message being answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Human-readable author, shown to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// `hello` params: the first request on a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloParams {
    /// The protocol version the client speaks, [`PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Who is connecting.
    pub client: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

/// `hello` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Hello {
    /// Protocol version the server speaks.
    pub protocol: u32,
    /// Server version.
    pub version: String,
    /// The selected environment.
    pub environment: String,
}
