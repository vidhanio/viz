//! Tool records, projects, worktrees, and harness catalogs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ConfigOption;

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
    /// Which mark to show: `claude`, `codex`, `gemini`, `omp`, `pi`, or `acp`
    /// for any other harness.
    pub icon: String,
    /// Whether it can run threads, as of `checked_at`.
    pub status: HarnessStatus,
    /// Unix milliseconds of the last check; `None` before the first.
    pub checked_at: Option<i64>,
    /// Session options advertised the last time the runner reported them:
    /// models with their groups and each model's thinking levels, then other
    /// options such as speed.
    pub options: Vec<ConfigOption>,
    /// The access modes a new thread can start in.
    pub access: Vec<crate::AccessMode>,
    /// The commands and skills its latest session offered.
    pub commands: Vec<crate::SlashCommand>,
}

/// `harness.sessions`: the sessions a harness saved for a project, or for
/// the default directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ListHarnessSessions {
    pub harness: String,
    pub project: Option<String>,
    /// From the previous page's `next`.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct HarnessSessions {
    /// Whether the harness can list its sessions at all.
    pub supported: bool,
    pub sessions: Vec<HarnessSession>,
    pub next: Option<String>,
}

/// A session a harness saved.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSession {
    pub id: String,
    pub cwd: String,
    pub title: Option<String>,
    /// The harness's own timestamp text.
    pub updated_at: Option<String>,
    /// The thread already bound to it, if any.
    pub thread: Option<crate::ThreadId>,
}

/// `thread.import`: open a harness's saved session as a thread, with the
/// history the harness replays.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ImportSession {
    pub harness: String,
    pub session: String,
    pub project: Option<String>,
    pub title: Option<String>,
}

/// Whether a harness can run threads. A check runs in the background; the
/// list answers from what the last one found.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HarnessStatus {
    /// Not checked yet.
    Unchecked,
    Ready,
    /// Its command is not on the server's `PATH`.
    NotInstalled {
        message: String,
    },
    /// It needs the user to sign in.
    SignedOut {
        message: String,
    },
    /// Its provider refuses work until the limit resets.
    LimitReached {
        message: String,
        /// The harness's own reset hint, such as `resets 8pm
        /// (America/Toronto)`.
        reset: Option<String>,
    },
    Failed {
        message: String,
    },
}
