//! Shared protocol v2 types for viz servers and clients.
//!
//! Each request's params type implements [`Method`], which names the method on
//! the wire and its result type.

mod history;

use std::fmt;

pub use history::{
    Attachment, AttachmentCapabilities, Entry, EntryData, EntryId, EntryPage, ListEntries,
    PlanEntry, PlanPriority, PlanStatus, PromptAttachment, QueuedInput, RemoveQueuedInput, RunEnd,
    SearchHit, SearchHits, SearchThreads, UpdateQueuedInput,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 2;

/// A thread id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ThreadId(pub u64);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A run id.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct RunId(pub u64);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A request method: its params type, its name on the wire, and its result
/// type. Sealed: the protocol defines every method.
pub trait Method: Serialize + Sync + private::Sealed {
    /// The JSON-RPC method name.
    const NAME: &'static str;
    /// The `result` of a successful call.
    type Response: DeserializeOwned;
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
            serde_json::json!({
                $($name: {
                    "params": schemars::schema_for!($params),
                    "result": schemars::schema_for!($response),
                },)*
            })
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
    WaitThread => "thread.wait", WaitResult;
    ReadThreadState => "thread.state", ThreadStateResult;
    ReadProject<'_> => "project.read", ProjectResult;
    UpdateQueuedInput<'_> => "queue.update", Empty;
    ListThreads<'_> => "threads.list", ThreadList;
    UpdateThread<'_> => "thread.update", Empty;
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

/// `threads.list` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListThreads<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
}

/// `threads.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadList {
    /// Every thread.
    pub threads: Vec<ThreadSummary>,
}

/// A `threads.list` item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    /// Its id.
    pub id: ThreadId,
    /// A user rename, an ACP session title, or a title from the first prompt.
    pub title: String,
    /// For a subagent: the thread that started it.
    pub parent: Option<ThreadId>,
    pub parent_environment: Option<String>,
    /// The project directory this thread belongs to, if any.
    pub project: Option<String>,
    /// Configuration key of the ACP harness.
    pub harness: String,
    /// Display name of the ACP harness.
    pub harness_name: String,
    /// What this thread is doing.
    pub status: ThreadStatus,
    /// Whether the thread is archived.
    pub archived: bool,
    /// Time of the last input or turn end in Unix milliseconds; zero if
    /// unknown.
    pub updated_at: i64,
    /// A turn settled while no app was watching, or this chat was manually
    /// marked unread.
    pub unread: bool,
    /// Show the thread in the pinned section.
    pub pinned: bool,
    /// Display name of the selected model, once reported.
    pub model: Option<String>,
    /// Display name of the selected thinking level, when the model has one.
    pub thinking: Option<String>,
}

/// `thread.launch` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LaunchThread {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Project directory; absent uses the default working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Configured runner; absent uses the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Initial configuration by option id.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub options: std::collections::BTreeMap<String, OptionValue>,
}

/// `thread.launch` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Launched {
    /// The new thread.
    pub thread: ThreadId,
    pub run: Option<RunId>,
}

/// Availability of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    /// Nothing is running.
    Idle,
    /// A turn is running or waiting.
    Working,
    /// The last turn failed.
    Failed,
}

/// Config option value, stored as a string choice or a boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum OptionValue {
    /// A named choice.
    Select(String),
    /// On or off.
    Boolean(bool),
}

/// The control used for a configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    /// A choice among values.
    Select,
    /// A boolean toggle.
    Boolean,
}

/// A choice offered by a select option.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct OptionChoice {
    /// Value to send.
    pub value: String,
    /// Display label.
    pub name: String,
    /// Optional explanation.
    pub description: Option<String>,
    /// Optional group label.
    pub group: Option<String>,
    /// Thinking levels reported for this model, if available.
    pub thinking: Option<DependentOption>,
}

/// A select setting that depends on the chosen model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct DependentOption {
    /// Identifier for `thread.configure`.
    pub id: String,
    /// Display label.
    pub name: String,
    /// Optional explanation.
    pub description: Option<String>,
    /// Default or current value, depending on the surrounding catalog.
    pub value: OptionValue,
    /// Available choices.
    pub choices: Vec<DependentChoice>,
}

/// A choice in a model-dependent setting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct DependentChoice {
    /// Value to send.
    pub value: String,
    /// Display label.
    pub name: String,
    /// Optional explanation.
    pub description: Option<String>,
}

/// A setting offered by the server or ACP session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ConfigOption {
    /// Identifier for `thread.configure`.
    pub id: String,
    /// Display label.
    pub name: String,
    /// Optional explanation.
    pub description: Option<String>,
    /// Semantic category supplied by the runner.
    pub category: Option<String>,
    /// Type of control.
    #[serde(rename = "type")]
    pub kind: OptionKind,
    /// Current value.
    pub value: OptionValue,
    /// Available choices, empty for a boolean.
    pub choices: Vec<OptionChoice>,
}

/// A slash command offered by the ACP session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct SlashCommand {
    /// Command name without the slash.
    pub name: String,
    /// What the command does.
    pub description: String,
    /// Optional argument hint.
    pub hint: Option<String>,
}

/// The current application state of a watched thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadState {
    /// Status of this thread.
    pub status: ThreadStatus,
    /// The configured ACP runner.
    pub harness: String,
    /// The containing project, if any.
    pub project: Option<String>,
    /// Working directory.
    pub cwd: String,
    /// Configuration reported by the runner.
    pub options: Vec<ConfigOption>,
    /// Available slash commands.
    pub commands: Vec<SlashCommand>,
    /// Inputs waiting for their turn.
    pub queue: Vec<QueuedInput>,
    /// What kind of attachment the current runner accepts directly.
    pub attachments: AttachmentCapabilities,
    /// An ACP turn's in-progress text and tools; `None` when none runs.
    pub live: Option<LiveTurn>,
    /// The ACP runner's last context usage report, if any.
    pub usage: Option<ContextUsage>,
}

/// The running ACP turn's uncommitted tail. The commit that appends it as
/// entries, or ends the turn, clears it; its text comes before its tools.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct LiveTurn {
    /// Entry id of the turn's active run, as a string.
    pub turn: String,
    /// The text run being streamed, if any.
    pub text: Option<LiveText>,
    /// Unfinished tool calls, in the order they started.
    pub tools: Vec<ToolRecord>,
}

/// A streamed text run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct LiveText {
    /// Stable key; also `data.key` of the entry that commits this run.
    pub key: String,
    /// Prose or thinking.
    pub kind: LiveTextKind,
    /// The text so far.
    pub text: String,
}

/// What a [`LiveText`] run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LiveTextKind {
    /// Agent prose.
    Text,
    /// Agent thinking.
    Thought,
}

/// An ACP tool call: the data of a tool entry, or an unfinished
/// call in [`LiveTurn::tools`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolRecord {
    /// ACP tool call id; unique within one turn.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Programmatic tool name, when reported by the runner.
    pub name: Option<String>,
    /// ACP tool kind, such as `execute` or `edit`.
    pub kind: Option<String>,
    /// Progress of the call.
    pub status: ToolCallStatus,
    /// The viz tool this call ran, if it was one.
    pub viz: Option<String>,
    /// The thread the call started, if any.
    pub child: Option<ThreadId>,
    pub child_environment: Option<String>,
    /// Unix milliseconds when viz first observed the call running.
    pub started_at: Option<i64>,
    /// Raw input: a string as sent, else JSON.
    pub input: Option<String>,
    /// ACP text content, else readable raw output.
    pub output: Option<String>,
    /// File changes.
    #[serde(default)]
    pub diffs: Vec<ToolDiff>,
    /// Paths the call touched.
    #[serde(default)]
    pub locations: Vec<String>,
}

/// Progress of an ACP tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Not started.
    Pending,
    /// Running.
    InProgress,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
}

/// One file change of a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiff {
    /// Changed file.
    pub path: String,
    /// Previous content; `None` for a new file.
    pub old_text: Option<String>,
    /// New content.
    pub new_text: String,
}

/// Context window usage reported by an ACP runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ContextUsage {
    /// Tokens currently in context.
    pub used: u64,
    /// Context window size in tokens.
    pub size: u64,
}

/// A working directory shared by a group of threads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Display name.
    pub name: String,
    /// Absolute directory path.
    pub path: String,
    /// Creation time in Unix milliseconds.
    pub created_at: i64,
}

/// A configured ACP runner.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Harness {
    /// Configuration key used to select this harness.
    pub id: String,
    /// Display name shown to the user.
    pub name: String,
    /// Whether this is the server default.
    pub default: bool,
    /// Session options advertised the last time the runner reported them.
    pub options: Vec<ConfigOption>,
}

/// Empty successful response or notification params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Empty {}

/// Change a thread's metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UpdateThread<'a> {
    /// The thread to change.
    pub thread: ThreadId,
    /// A new title, if changing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    /// A new archive status, if changing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// A new pinned status, if changing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    /// A new unread status, if changing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
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
pub struct DeleteThread {
    /// The thread to delete.
    pub thread: ThreadId,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct UpdateProject<'a> {
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
    /// Matching relative paths.
    pub files: Vec<String>,
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

/// `thread.watch` params: start receiving
/// [`Notification::ThreadView`] for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Watch {
    /// The thread.
    pub thread: ThreadId,
}

/// `thread.unwatch` params: stop watching a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unwatch {
    /// The thread.
    pub thread: ThreadId,
}

/// `thread.watch` and `thread.unwatch` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Watching {
    /// Whether this client now watches the thread.
    pub watching: bool,
}

/// Notifications pushed after hello.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Notification {
    #[serde(rename = "reply")]
    Reply {
        origin: Origin,
        thread: ThreadId,
        run: RunId,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "thread.view")]
    ThreadView {
        thread: ThreadId,
        entries: Vec<Entry>,
        reset: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<EntryId>,
    },
    #[serde(rename = "thread.state")]
    ThreadState {
        thread: ThreadId,
        state: ThreadState,
    },
    #[serde(rename = "threads.changed")]
    ThreadsChanged(Empty),
    #[serde(rename = "tasks.changed")]
    TasksChanged(Empty),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListEnvironments {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct EnvironmentList {
    pub environments: Vec<Environment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Environment {
    pub id: String,
    pub url: Option<String>,
    pub status: EnvironmentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentStatus {
    Local,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitThread {
    pub thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitResult {
    pub run: Option<Run>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Run {
    pub id: RunId,
    pub thread: ThreadId,
    /// What started the run.
    pub kind: RunKind,
    pub status: RunStatus,
    pub answer: Option<String>,
    pub error: Option<String>,
}

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    /// An input: a prompt entry starts it and the ACP prompt response ends it.
    Prompt,
    /// The agent continued on its own after its turn ended, such as when a
    /// backgrounded command finished. A `continued` entry starts it; the
    /// server ends it when the agent goes quiet or an input is waiting.
    Unprompted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadThreadState {
    pub thread: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadStateResult {
    pub state: ThreadState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ReadProject<'a> {
    pub project: &'a str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTasks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct TaskList {
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Task {
    pub id: String,
    pub parent: ThreadId,
    pub environment: String,
    pub thread: Option<ThreadId>,
    pub run: Option<RunId>,
    pub status: TaskStatus,
    pub reachable: bool,
    pub title: String,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusRequest<'a> {
    pub task: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusResult {
    pub task: Task,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CancelTask<'a> {
    pub task: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct TaskResult {
    pub task: Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DelegateTask<'a> {
    pub parent: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    pub text: &'a str,
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub attachments: &'a [Attachment],
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub attachment_ids: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<&'a str>,
}

const fn slice_is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}
