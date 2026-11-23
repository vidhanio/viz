//! Tool records, projects, worktrees, and harness catalogs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg(doc)]
use super::LiveTurn;
use super::{ConfigOption, ThreadId};

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
    /// Project preferences shared by all clients.
    pub settings: crate::ProjectSettings,
    /// Display name.
    pub name: String,
    /// Absolute directory path.
    pub path: String,
    /// Creation time in Unix milliseconds.
    pub created_at: i64,
    /// Whether the directory is inside a Git checkout, so its threads can
    /// run in worktrees.
    pub git: bool,
    /// The checkout's current branch; `None` outside Git or on a detached
    /// HEAD.
    pub branch: Option<String>,
}

/// A linked Git worktree a thread runs in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadWorktree {
    /// Absolute path of the worktree root.
    pub path: String,
    /// Its current branch; `None` on a detached HEAD or once removed.
    pub branch: Option<String>,
}

/// Where in a project's Git repository a thread runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum WorktreeTarget {
    /// The project's own checkout.
    Local,
    /// A new linked worktree on a new branch.
    New {
        /// Branch, remote-tracking branch or commit to start from; defaults
        /// to the project checkout's HEAD.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        /// New branch name; defaults to `viz/` and a slug of the thread title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
    /// An existing linked worktree of the project's repository.
    Existing {
        /// Absolute path of the worktree root.
        path: String,
    },
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
