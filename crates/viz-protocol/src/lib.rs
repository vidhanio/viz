//! Shared protocol v2 types for viz servers and clients.
//!
//! Each request's params type implements [`Method`], which names the method on
//! the wire and its result type.

mod catalog;
mod connections;
pub mod content;
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
    Watching, WorkspaceChange,
};
pub use content::{
    CONTENT_PROTOCOL_VERSION, ConnectionChannel, ContentFrameError, ContentRequest,
    ContentResponse, ContentVariant, decode_content_frame, decode_content_frame_offset,
    encode_content_frame,
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
    AttachmentCapabilities, BlobAttachment, Entry, EntryAnchor, EntryData, EntryPage, ListEntries,
    PlanEntry, PlanPriority, PlanStatus, PromptAttachment, PromptContext, QueuedInput,
    RemoveQueuedInput, RunEnd, SearchHit, SearchHits, SearchThreads, SteerQueuedInput, TurnTokens,
    TurnUsage, UpdateQueuedInput, UsageCost, attachment_inline_md, attachment_ref_labels,
    text_references_label,
};
pub use ids::{EntryId, HostId, RequestId, RunId, TaskId, ThreadId};
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
    DefaultCheckout, ProjectAutoSettle, ProjectIcon, ProjectIconChange, ProjectSettings,
    ProjectSettingsPatch, Setting,
};
pub use requests::{
    BlobInfo, BranchList, ConfigureThread, CreateProject, DeleteThread, Empty, FileBody,
    FileContent, FileList, FileRoot, FileSearchKind, FileSearchMatch, GitBranch, HarnessList,
    HarnessOptions, HarnessOptionsResult, LineWindow, ListBranches, ListHarnesses, ListProjects,
    ListWorktrees, MoveThread, Moved, Patch, ProjectList, ProjectResult, PromoteThread,
    PromotedThread, ReadFile, RemoveProject, RemoveWorktree, SearchFiles, SendInput, SendMode,
    Sent, StatBlob, StopThread, UpdateProject, UpdateThread, Worktree, WorktreeList,
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
    MediaBlob, Plan, PlanContent, Segment, TextKind, TranscriptEntry,
};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 3;

/// Capability bitmask declaring which authenticated caller classes may invoke
/// a protocol method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodScope(u8);

impl MethodScope {
    /// Chat/messaging connector WebSocket (`ClientKind::Connector`).
    pub const CONNECTOR: Self = Self(1 << 1);
    /// Initial connection handshake (`hello`).
    pub const HANDSHAKE: Self = Self(1 << 4);
    /// Full operator session (local Unix socket, authenticated TCP client, or
    /// peer-proxied desktop/CLI WebSocket).
    pub const OPERATOR: Self = Self(1 << 0);
    /// Server-to-server durable delegation RPC over authenticated peer HTTP.
    pub const PEER_DELEGATION: Self = Self(1 << 2);
    /// Cross-environment workspace RPC invoked by a peer's MCP tools over
    /// authenticated peer HTTP.
    pub const PEER_TOOL: Self = Self(1 << 3);

    /// Combines two capability bitmasks.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether `self` and `other` share at least one capability bit.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    /// Human-readable scope labels emitted into `protocol/schema.json`.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.intersects(Self::HANDSHAKE) {
            out.push("handshake");
        }
        if self.intersects(Self::OPERATOR) {
            out.push("operator");
        }
        if self.intersects(Self::CONNECTOR) {
            out.push("connector");
        }
        if self.intersects(Self::PEER_TOOL) {
            out.push("peer_tool");
        }
        if self.intersects(Self::PEER_DELEGATION) {
            out.push("peer_delegation");
        }
        out
    }
}

/// A request method: its params type, its name on the wire, its required
/// capability scope, and its result type. Sealed: the protocol defines every
/// method.
pub trait Method: Serialize + DeserializeOwned + Send + Sync + private::Sealed {
    /// The JSON-RPC method name.
    const NAME: &'static str;
    /// Caller scopes allowed to invoke this method.
    const SCOPE: MethodScope;
    /// The `result` of a successful call.
    type Response: Clone + DeserializeOwned + Serialize;
}

mod private {
    pub trait Sealed {}
}

const OP: MethodScope = MethodScope::OPERATOR;
const OP_CONN: MethodScope = MethodScope::OPERATOR.union(MethodScope::CONNECTOR);
const OP_PEER: MethodScope = MethodScope::OPERATOR.union(MethodScope::PEER_TOOL);
const OP_CONN_PEER: MethodScope = OP_CONN.union(MethodScope::PEER_TOOL);

macro_rules! methods {
    ($($params:ty => $name:literal, $response:ty, $scope:expr;)*) => {
        $(
        impl private::Sealed for $params {}
        impl Method for $params {
            const NAME: &'static str = $name;
            const SCOPE: MethodScope = $scope;
            type Response = $response;
        }
        )*

        /// Input and output schemas for every public wire method.
        #[must_use]
        pub fn method_schemas() -> serde_json::Value {
            serde_json::Value::Object([
                $(($name.to_owned(), serde_json::json!({
                    "scopes": (<$params as Method>::SCOPE).names(),
                    "params": schemars::schema_for!($params),
                    "result": schemars::schema_for!($response),
                })),)*
            ].into_iter().collect())
        }
    };
}

methods! {
    HelloParams => "hello", Hello, MethodScope::HANDSHAKE;
    ListEnvironments => "environments.list", EnvironmentList, OP;
    ListTasks => "tasks.list", TaskList, OP;
    TaskStatusRequest => "task.status", TaskStatusResult, OP;
    CancelTask => "task.cancel", TaskResult, OP;
    DelegateTask => "task.delegate", TaskResult, OP;
    DeleteTask => "task.delete", Empty, OP;
    ClearTasks => "tasks.clear", Cleared, OP;
    WaitThread => "thread.wait", WaitResult, OP_CONN_PEER;
    ReadThreadState => "thread.state", ThreadStateResult, OP_CONN_PEER;
    ReadProject => "project.read", ProjectResult, OP;
    SteerQueuedInput => "queue.steer", Empty, OP;
    UpdateQueuedInput => "queue.update", Empty, OP;
    ListThreads => "threads.list", ThreadList, OP_CONN_PEER;
    UpdateThread => "thread.update", ThreadUpdated, OP_PEER;
    RetitleThread => "thread.retitle", ThreadUpdated, OP_PEER;
    TriageThread => "thread.triage", Triaged, OP_PEER;
    ConfigureThread => "thread.configure", Empty, OP;
    StopThread => "thread.stop", Empty, OP_CONN_PEER;
    DeleteThread => "thread.delete", Empty, OP;
    ListProjects => "projects.list", ProjectList, OP_CONN_PEER;
    CreateProject => "project.create", ProjectResult, OP_PEER;
    UpdateProject => "project.update", Empty, OP_PEER;
    BrowseFolders => "folders.browse", FolderEntries, OP;
    CreateFolder => "folder.create", FolderCreated, OP;
    GitStatusRequest => "git.status", GitStatus, OP;
    GitActionRequest => "git.action", GitActionResult, OP;
    RestoreCheckpoint => "git.restoreCheckpoint", Empty, OP;
    GitDiffRequest => "git.diff", GitDiff, OP;
    WatchGit => "git.watch", GitStatus, OP;
    UnwatchGit => "git.unwatch", Empty, OP;
    InitRepository => "git.init", FolderCreated, OP;
    StartClone => "project.clone", CloneStatus, OP;
    ReadClone => "project.cloneStatus", CloneStatus, OP;
    CancelClone => "project.cancelClone", CloneStatus, OP;
    GitHubAuth => "github.auth", GitHubAuthState, OP;
    GitHubSearch => "github.search", GitHubSearchResult, OP;
    GitHubChecks => "github.checks", Vec<GitHubCheck>, OP;
    GitHubPullPage => "github.reviews", Vec<GitHubReview>, OP;
    GitHubCreatePull => "github.createPull", GitHubItem, OP;
    GitHubLaunch => "github.launch", Launched, OP;
    GitHubRepositories => "github.repositories", GitHubRepositoryList, OP;
    RemoveProject => "project.remove", Empty, OP_PEER;
    ListHarnesses => "harnesses.list", HarnessList, OP_CONN_PEER;
    HarnessOptions => "harnesses.options", HarnessOptionsResult, OP_PEER;
    ListHarnessSessions => "harness.sessions", HarnessSessions, OP;
    ImportSession => "thread.import", Launched, OP;
    SearchFiles => "files.search", FileList, OP;
    ReadFile => "files.read", FileContent, OP;
    StatBlob => "blob.stat", BlobInfo, OP_CONN;
    LaunchThread => "thread.launch", Launched, OP_CONN_PEER;
    ForkThread => "thread.fork", Forked, OP_PEER;
    SendInput => "thread.send", Sent, OP_CONN_PEER;
    Watch => "thread.watch", Watching, OP_CONN;
    Unwatch => "thread.unwatch", Watching, OP_CONN;
    SearchThreads => "threads.search", SearchHits, OP_PEER;
    ListEntries => "thread.entries", EntryPage, OP_CONN_PEER;
    AnswerRequest => "thread.answer", RunRequest, OP;
    SetThreadAccess => "thread.access", Empty, OP;
    SetProjectAccess => "project.access", Empty, OP;
    RemoveQueuedInput => "queue.remove", Empty, OP;
    ListBranches => "project.branches", BranchList, OP_PEER;
    ListWorktrees => "worktrees.list", WorktreeList, OP_PEER;
    RemoveWorktree => "worktree.remove", Empty, OP_PEER;
    MoveThread => "thread.move", Moved, OP_PEER;
    PromoteThread => "thread.promoteProject", PromotedThread, OP;
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
    /// Traffic class for this WebSocket connection (defaults to control).
    #[serde(default, skip_serializing_if = "ConnectionChannel::is_control")]
    pub channel: ConnectionChannel,
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
    /// Identity of the machine serving workspace paths.
    pub host: HostId,
}
