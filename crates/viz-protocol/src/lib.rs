//! Shared protocol v2 types for viz servers and clients.
//!
//! Each request's params type implements [`Method`], which names the method on
//! the wire and its result type.

mod catalog;
mod connections;
mod history;
mod ids;
mod lifecycle;
pub mod peer;
mod requests;
mod threads;

pub use catalog::{
    ContextUsage, Harness, Project, ThreadWorktree, ToolCallStatus, ToolDiff, ToolRecord,
    WorktreeTarget,
};
pub use connections::{
    CancelTask, ClearTasks, Cleared, DelegateTask, DeleteTask, Environment, EnvironmentList,
    EnvironmentStatus, ListEnvironments, ListTasks, Notification, ReadProject, ReadThreadState,
    Run, RunKind, RunStatus, Task, TaskList, TaskResult, TaskStatus, TaskStatusRequest,
    TaskStatusResult, ThreadStateResult, Unwatch, WaitResult, WaitThread, Watch, Watching,
};
pub use history::{
    Attachment, AttachmentCapabilities, Entry, EntryData, EntryPage, ListEntries, PlanEntry,
    PlanPriority, PlanStatus, PromptAttachment, QueuedInput, RemoveQueuedInput, RunEnd, SearchHit,
    SearchHits, SearchThreads, SteerQueuedInput, UpdateQueuedInput,
};
pub use ids::{EntryId, RunId, TaskId, ThreadId};
pub use lifecycle::{
    Attention, AutoSettle, Lifecycle, Position, RetitleThread, SettledBy, Shelf, Standing,
    ThreadUpdated, TriageAction, TriageThread, Triaged,
};
pub use requests::{
    BranchList, ConfigureThread, CreateProject, DeleteThread, Empty, FileList, GitBranch,
    HarnessList, HarnessOptions, HarnessOptionsResult, ListBranches, ListHarnesses, ListProjects,
    ListWorktrees, MoveThread, Moved, ProjectList, ProjectResult, RemoveProject, RemoveWorktree,
    SearchFiles, SendInput, SendMode, Sent, StopThread, UpdateProject, UpdateThread, Worktree,
    WorktreeList,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
pub use threads::{
    ConfigOption, DependentChoice, DependentOption, LaunchThread, Launched, ListThreads, LiveText,
    LiveTextKind, LiveTurn, OptionChoice, OptionKind, OptionValue, SlashCommand, ThreadList,
    ThreadState, ThreadStatus, ThreadSummary,
};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// A request method: its params type, its name on the wire, and its result
/// type. Sealed: the protocol defines every method.
pub trait Method: Serialize + Sync + private::Sealed {
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
    TaskStatusRequest<'_> => "task.status", TaskStatusResult;
    CancelTask<'_> => "task.cancel", TaskResult;
    DelegateTask<'_> => "task.delegate", TaskResult;
    DeleteTask<'_> => "task.delete", Empty;
    ClearTasks => "tasks.clear", Cleared;
    WaitThread => "thread.wait", WaitResult;
    ReadThreadState => "thread.state", ThreadStateResult;
    ReadProject<'_> => "project.read", ProjectResult;
    SteerQueuedInput => "queue.steer", Empty;
    UpdateQueuedInput<'_> => "queue.update", Empty;
    ListThreads<'_> => "threads.list", ThreadList;
    UpdateThread<'_> => "thread.update", ThreadUpdated;
    RetitleThread => "thread.retitle", ThreadUpdated;
    TriageThread => "thread.triage", Triaged;
    ConfigureThread<'_> => "thread.configure", Empty;
    StopThread => "thread.stop", Empty;
    DeleteThread => "thread.delete", Empty;
    ListProjects => "projects.list", ProjectList;
    CreateProject<'_> => "project.create", ProjectResult;
    UpdateProject<'_> => "project.update", Empty;
    RemoveProject<'_> => "project.remove", Empty;
    ListHarnesses => "harnesses.list", HarnessList;
    HarnessOptions => "harnesses.options", HarnessOptionsResult;
    SearchFiles<'_> => "files.search", FileList;
    LaunchThread => "thread.launch", Launched;
    SendInput => "thread.send", Sent;
    Watch => "thread.watch", Watching;
    Unwatch => "thread.unwatch", Watching;
    SearchThreads<'_> => "threads.search", SearchHits;
    ListEntries => "thread.entries", EntryPage;
    RemoveQueuedInput => "queue.remove", Empty;
    ListBranches<'_> => "project.branches", BranchList;
    ListWorktrees<'_> => "worktrees.list", WorktreeList;
    RemoveWorktree<'_> => "worktree.remove", Empty;
    MoveThread => "thread.move", Moved;
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
