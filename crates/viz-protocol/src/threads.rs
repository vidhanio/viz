//! Thread creation, configuration, summaries, and live transcript state.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Attachment, AttachmentCapabilities, Attention, ContextUsage, ForkSource, Lifecycle, LiveTurn,
    QueuedInput, RunId, Shelf, ThreadId, ThreadWorktree, WorktreeTarget,
};

/// `threads.list` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListThreads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// `threads.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadList {
    /// Every thread, in the order of [`Shelf`] and of each shelf.
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
    /// The thread this one continues a copy of.
    pub forked_from: Option<ForkSource>,
    /// The project directory this thread belongs to, if any.
    pub project: Option<String>,
    /// The project's linked Git worktree the thread runs in; `None` in the
    /// project's own checkout.
    pub worktree: Option<ThreadWorktree>,
    /// The most recently observed pull request for this checkout branch.
    pub pull_request: Option<crate::GitHubPull>,
    /// Configuration key of the ACP harness.
    pub harness: String,
    /// Display name of the ACP harness.
    pub harness_name: String,
    /// What this thread is doing.
    pub status: ThreadStatus,
    /// Where it stands in the user's triage.
    pub lifecycle: Lifecycle,
    /// The part of the list it belongs in.
    pub shelf: Shelf,
    /// Why it wants the user, if it does.
    pub attention: Option<Attention>,
    /// Delegated tasks still under way.
    pub waiting_on: u32,
    /// The user's latest input, in Unix milliseconds.
    pub last_input_at: Option<i64>,
    /// Time of the last input or turn end in Unix milliseconds; zero if
    /// unknown.
    pub updated_at: i64,
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
    /// Files and links the first input mentions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<crate::PromptContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Project directory. Omit to create a private scratch directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Configured runner; absent uses the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Where in the project's repository to run. Omit to use the project's
    /// default checkout mode. Requires `project`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeTarget>,
    /// Access override applied before the first input runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::AccessMode>,
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
    #[serde(default)]
    pub source: CommandSource,
}

/// What a slash command runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    /// One of the harness's own commands, or one it does not say more about.
    #[default]
    Command,
    /// A skill, for `$`.
    Skill,
    /// A prompt from an MCP server.
    Mcp,
}

/// The current application state of a watched thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadState {
    /// Status of this thread.
    pub status: ThreadStatus,
    /// The configured ACP runner.
    pub harness: String,
    /// The containing project, if any.
    pub project: Option<String>,
    /// The linked Git worktree the thread runs in, if any.
    pub worktree: Option<ThreadWorktree>,
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
    /// The access mode and the modes the harness offers.
    pub access: crate::Access,
    /// Approvals and questions awaiting an answer, oldest first.
    pub requests: Vec<crate::RunRequest>,
}
