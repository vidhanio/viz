//! Protocol v1 types, mirroring `backend/src/protocol.ts` (the source of truth;
//! its JSON Schema is `protocol/schema.json`).
//!
//! Each request's params type implements [`Method`], which names the method on
//! the wire and its result type.

mod workspace;

use std::fmt;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub use workspace::{
    Attachment, AttachmentCapabilities, Branches, CommitResult, DiffFile, DiffFileStatus,
    DiffFiles, DiffScope, EntryId, EntryPage, GitBranches, GitCheckout, GitCommit, GitDiff,
    GitFile, GitFileStatus, GitPr, GitPush, GitRevert, GitState, ListEntries, PrResult,
    QueuedInput, RemoveQueuedInput, SearchConversations, SearchHit, SearchHits, Workspace,
};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// A pi-durable conversation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(pub u64);

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A pi-durable submission id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubmissionId(pub u64);

impl fmt::Display for SubmissionId {
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
    ($($params:ty => $name:literal, $response:ty;)*) => {$(
        impl private::Sealed for $params {}
        impl Method for $params {
            const NAME: &'static str = $name;
            type Response = $response;
        }
    )*};
}

methods! {
    HelloParams => "hello", Hello;
    ListConversations => "conversations.list", ConversationList;
    UpdateConversation<'_> => "conversation.update", Empty;
    ConfigureConversation<'_> => "conversation.configure", Empty;
    AbortConversation => "conversation.abort", Empty;
    DeleteConversation => "conversation.delete", Deleted;
    ListProjects => "projects.list", ProjectList;
    CreateProject<'_> => "project.create", ProjectResult;
    UpdateProject<'_> => "project.update", ProjectResult;
    RemoveProject<'_> => "project.remove", Empty;
    ListWorktrees<'_> => "worktrees.list", WorktreeList;
    ListHarnesses => "harnesses.list", HarnessList;
    SearchFiles<'_> => "files.search", FileList;
    // `CreateConversation` owns its initial settings for callers that queue creation.
    CreateConversation => "conversation.create", Created;
    Submit => "conversation.submit", Submitted;
    Watch => "conversation.watch", Watching;
    Unwatch => "conversation.unwatch", Watching;
    SearchConversations<'_> => "conversations.search", SearchHits;
    ListEntries => "conversation.entries", EntryPage;
    RemoveQueuedInput => "queue.remove", Empty;
    GitDiff<'_> => "git.diff", DiffFiles;
    GitBranches<'_> => "git.branches", Branches;
    GitCheckout<'_> => "git.checkout", Empty;
    GitCommit<'_> => "git.commit", CommitResult;
    GitPush => "git.push", Empty;
    GitPr<'_> => "git.pr", PrResult;
    GitRevert => "git.revert", Empty;
}

/// What kind of client is connecting; decides which notifications it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    /// Full UI (desktop, mobile): watches whole conversations.
    App,
    /// Connector (Discord, iMessage, ...): receives replies to its own inputs
    /// only.
    Connector,
    /// One-off command-line client.
    Cli,
}

/// Identifies a client in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client kind.
    pub kind: ClientKind,
    /// Client name; for connectors, the name inputs' [`Origin::connector`] use.
    pub name: String,
}

/// Where an input came from, so its reply is routed back there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HelloParams {
    /// The protocol version the client speaks, [`PROTOCOL_VERSION`].
    pub protocol: u32,
    /// Who is connecting.
    pub client: ClientInfo,
}

/// `hello` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hello {
    /// Protocol version the backend speaks.
    pub protocol: u32,
    /// Backend version.
    pub version: String,
    /// The orchestrator's conversation id.
    pub root: ConversationId,
}

/// `conversations.list` params.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ListConversations {}

/// `conversations.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConversationList {
    /// Every conversation.
    pub conversations: Vec<ConversationSummary>,
}

/// What a conversation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationKind {
    /// The orchestrator: the root conversation.
    Orchestrator,
    /// A chat started from an app.
    Chat,
    /// A subagent, owned by a tool call of `parent`.
    Subagent,
}

/// A `conversations.list` item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    /// Its id.
    pub id: ConversationId,
    /// Set by the backend: fixed for the orchestrator, a subagent's task, a
    /// chat's first message; empty until known.
    pub title: String,
    /// What it is.
    pub kind: ConversationKind,
    /// For a subagent: the conversation whose tool call owns it.
    #[serde(default)]
    pub parent: Option<ConversationId>,
    /// The project directory this conversation belongs to, if any.
    pub project: Option<String>,
    /// The configured ACP runner, if any.
    pub harness: Option<String>,
    /// What this conversation is doing.
    pub status: ConversationStatus,
    /// Whether the conversation is archived.
    pub archived: bool,
    /// Time of the last input or turn end in Unix milliseconds; zero if
    /// unknown.
    pub updated_at: i64,
    /// A turn settled while no app was watching, or this chat was manually
    /// marked unread.
    pub unread: bool,
    /// Show the conversation in the pinned section.
    pub pinned: bool,
    /// The managed worktree path, if the chat uses one.
    pub worktree: Option<String>,
    /// The worktree's checked-out branch, absent when detached.
    pub branch: Option<String>,
}

/// `conversation.create` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CreateConversation {
    /// Project directory; absent uses the default working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Workspace selection; absent uses the project's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// Configured runner; absent uses the backend default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Initial configuration by option id.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub options: std::collections::BTreeMap<String, OptionValue>,
}

/// `conversation.create` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Created {
    /// The new conversation.
    pub conversation: ConversationId,
}

/// Availability of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationStatus {
    /// Nothing is running.
    Idle,
    /// A turn is running or waiting.
    Working,
    /// The last turn failed.
    Failed,
}

/// Config option value, stored as a string choice or a boolean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionValue {
    /// A named choice.
    Select(String),
    /// On or off.
    Boolean(bool),
}

/// The control used for a configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    /// A choice among values.
    Select,
    /// A boolean toggle.
    Boolean,
}

/// A choice offered by a select option.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OptionChoice {
    /// Value to send.
    pub value: String,
    /// Display label.
    pub name: String,
    /// Optional explanation.
    pub description: Option<String>,
    /// Optional group label.
    pub group: Option<String>,
}

/// A setting offered by the backend or ACP session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigOption {
    /// Identifier for `conversation.configure`.
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SlashCommand {
    /// Command name without the slash.
    pub name: String,
    /// What the command does.
    pub description: String,
    /// Optional argument hint.
    pub hint: Option<String>,
}

/// The current application state of a watched conversation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationState {
    /// Status of this conversation.
    pub status: ConversationStatus,
    /// The configured ACP runner, if any.
    pub harness: Option<String>,
    /// The containing project, if any.
    pub project: Option<String>,
    /// Working directory, if known.
    pub cwd: Option<String>,
    /// Whether input is refused.
    pub read_only: bool,
    /// Configuration reported by the runner or orchestrator.
    pub options: Vec<ConfigOption>,
    /// Available slash commands.
    pub commands: Vec<SlashCommand>,
    /// Inputs waiting for their turn.
    pub queue: Vec<QueuedInput>,
    /// Git state of the current working directory.
    pub git: Option<GitState>,
    /// What kind of attachment the current runner accepts directly.
    pub attachments: AttachmentCapabilities,
}

/// A working directory shared by a group of chats.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Backend-assigned id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Absolute directory path.
    pub path: String,
    /// Checked-out Git branch, if attached on a repository.
    pub branch: Option<String>,
    /// Workspace mode for new chats.
    pub default_workspace: DefaultWorkspace,
    /// Creation time in Unix milliseconds.
    pub created_at: i64,
}

/// A project's default workspace for new chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultWorkspace {
    /// Use the project's existing checkout.
    Local,
    /// Make a separate checkout for each chat.
    Worktree,
}

/// A configured ACP runner.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Harness {
    /// Configured name.
    pub name: String,
    /// Whether this is the backend default.
    pub default: bool,
    /// Session options advertised the last time the runner reported them.
    pub options: Vec<ConfigOption>,
}

/// Empty successful response or notification params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Empty {}

/// Change a conversation's metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UpdateConversation<'a> {
    /// The conversation to change.
    pub conversation: ConversationId,
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

/// Set a conversation's configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConfigureConversation<'a> {
    /// The conversation to configure.
    pub conversation: ConversationId,
    /// The option id.
    pub option: &'a str,
    /// The new option value.
    pub value: &'a OptionValue,
}

/// Stop a conversation's active turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AbortConversation {
    /// The conversation to stop.
    pub conversation: ConversationId,
}

/// Delete a conversation and its subagents. The orchestrator can't be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeleteConversation {
    /// The conversation to delete.
    pub conversation: ConversationId,
}

/// Result of `conversation.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Deleted {
    /// Worktree paths left on disk because they have uncommitted changes.
    pub kept: Vec<String>,
}

/// List projects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ListProjects {}

/// Projects returned by `projects.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProjectList {
    /// All projects.
    pub projects: Vec<Project>,
}

/// Add a project directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CreateProject<'a> {
    /// Existing directory; `~` expands to the backend's home directory.
    pub path: &'a str,
    /// Display name; omit to use the directory name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
}

/// Update a project's display name or default workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UpdateProject<'a> {
    /// Project id.
    pub project: &'a str,
    /// New display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// New default workspace.
    #[serde(rename = "defaultWorkspace", skip_serializing_if = "Option::is_none")]
    pub default_workspace: Option<DefaultWorkspace>,
}

/// The project returned after creation or an update.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProjectResult {
    /// The resulting project.
    pub project: Project,
}

/// Remove a project directory from the project list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RemoveProject<'a> {
    /// Project id.
    pub project: &'a str,
}

/// List managed worktrees available to reuse in a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ListWorktrees<'a> {
    /// Project id.
    pub project: &'a str,
}

/// Managed worktrees, newest conversation first.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorktreeList {
    /// One entry per worktree.
    pub worktrees: Vec<ManagedWorktree>,
}

/// A managed worktree and the latest conversation that uses it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorktree {
    /// Full path to the checkout.
    pub path: String,
    /// Its current branch, absent when detached.
    pub branch: Option<String>,
    /// The latest live conversation using the checkout.
    pub conversation: ConversationId,
    /// Time of that conversation's last activity in Unix milliseconds.
    pub updated_at: i64,
}

/// List configured ACP runners.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ListHarnesses {}

/// ACP runners returned by `harnesses.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HarnessList {
    /// Configured ACP runners.
    pub harnesses: Vec<Harness>,
}

/// Search a conversation's working directory for files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SearchFiles<'a> {
    /// Conversation whose working directory is searched.
    pub conversation: ConversationId,
    /// Fuzzy path query.
    pub query: &'a str,
    /// Limit from 1 to 200; omit to use the backend default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// Search results from `files.search`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FileList {
    /// Matching relative paths.
    pub files: Vec<String>,
}

/// `conversation.submit` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Submit {
    /// Target conversation; `None` is the orchestrator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationId>,
    /// The input text.
    pub text: String,
    /// Idempotency key: a retry with the same id returns the same submission.
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Where the input came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// Files to attach to this input.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// `conversation.submit` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Submitted {
    /// The submission the input became.
    pub submission: SubmissionId,
}

/// `conversation.watch` params: start receiving
/// [`Notification::ConversationView`] for a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Watch {
    /// The conversation.
    pub conversation: ConversationId,
}

/// `conversation.unwatch` params: stop watching a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Unwatch {
    /// The conversation.
    pub conversation: ConversationId,
}

/// `conversation.watch` and `conversation.unwatch` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Watching {
    /// Whether this client now watches the conversation.
    pub watching: bool,
}

/// How a submission settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettledStatus {
    /// Answered.
    Done,
    /// Settled without an answer (aborted, failed, or nothing to answer).
    Unanswered,
}

/// A notification from the backend.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Notification {
    /// The answer to an input this connector submitted.
    #[serde(rename = "reply")]
    Reply {
        /// The input's origin.
        origin: Origin,
        /// The conversation it was submitted to.
        conversation: ConversationId,
        /// The submission.
        submission: SubmissionId,
        /// How it settled.
        status: SettledStatus,
        /// The answer's text, if any.
        text: Option<String>,
    },
    /// A revision of a watched conversation's view.
    #[serde(rename = "conversation.view")]
    ConversationView(ViewRevision),
    /// A full update of the application's state for a watched conversation.
    #[serde(rename = "conversation.state")]
    ConversationState {
        /// The conversation that changed.
        conversation: ConversationId,
        /// Its latest state.
        state: ConversationState,
    },
    /// Refresh the conversation and project lists.
    #[serde(rename = "conversations.changed")]
    ConversationsChanged(Empty),
}

/// `conversation.view` params: one revision of a watched conversation's view.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawViewRevision")]
pub struct ViewRevision {
    /// The conversation.
    pub conversation: ConversationId,
    /// The revision's content.
    pub revision: Revision,
}

/// What a [`ViewRevision`] carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revision {
    /// The full view: the first revision, and any that replace the replica.
    Full(Value),
    /// Chord delta ops against the previous revision (see
    /// [`delta`](crate::delta)).
    Delta(Vec<Value>),
}

/// `conversation.view` params as sent: `value` or `ops`.
#[derive(Deserialize)]
struct RawViewRevision {
    conversation: ConversationId,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    ops: Option<Vec<Value>>,
}

impl TryFrom<RawViewRevision> for ViewRevision {
    type Error = &'static str;

    fn try_from(raw: RawViewRevision) -> Result<Self, Self::Error> {
        let revision = match (raw.value, raw.ops) {
            (Some(value), _) => Revision::Full(value),
            (None, Some(ops)) => Revision::Delta(ops),
            (None, None) => return Err("a view revision has neither `value` nor `ops`"),
        };
        Ok(Self {
            conversation: raw.conversation,
            revision,
        })
    }
}
