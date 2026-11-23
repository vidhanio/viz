//! Where a thread stands in the user's triage, and how to move it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ThreadId;

/// A thread's place in triage. Every change returns the lifecycle before it,
/// which [`TriageAction::Restore`] puts back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lifecycle {
    pub standing: Standing,
    /// Put off until this time, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<i64>,
    /// Position among pinned threads, if pinned; lower is higher up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<i64>,
    /// Position among active threads, if the user arranged it. Threads without
    /// one come first, the latest input on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    /// Whether it settles by itself.
    #[serde(default)]
    pub auto_settle: AutoSettle,
    pub archived: bool,
    /// A turn settled while no app was watching, or the user marked it.
    pub unread: bool,
}

/// Whether the user is done with a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Standing {
    /// Open, and settled by the usual rules.
    #[default]
    Open,
    /// Brought back by the user or a due snooze: it does not settle by itself
    /// until it gets new input.
    Kept,
    /// Done. New input or agent activity opens it again.
    Settled {
        /// Unix milliseconds.
        at: i64,
        by: SettledBy,
    },
}

/// What settled a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SettledBy {
    User,
    Inactivity,
    /// Its pull request was merged or closed.
    PullRequest,
}

/// A thread's say in automatic settlement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AutoSettle {
    /// By the usual rules.
    #[default]
    Follow,
    Never,
}

/// The part of the thread list a thread belongs in.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Shelf {
    Pinned,
    /// Running, queued, or waiting for work it delegated.
    Working,
    Active,
    Snoozed,
    Settled,
    Archived,
}

/// Why a thread wants the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Attention {
    /// The agent awaits a permission.
    Approval,
    /// The agent awaits an answer.
    Question,
    /// Its last run failed, unseen.
    Failure,
    /// Something finished, unseen.
    Unread,
}

/// Where to put a thread in its list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "at", rename_all = "lowercase")]
pub enum Position {
    #[default]
    Top,
    Before {
        thread: ThreadId,
    },
    Bottom,
}

/// A change to a thread's [`Lifecycle`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum TriageAction {
    /// Done with it: clears its pin, position and snooze. Refused while it
    /// works.
    Settle,
    /// Not done after all.
    Reopen,
    /// Put it off.
    Snooze {
        /// Unix milliseconds, in the future.
        until: i64,
    },
    /// End a snooze.
    Wake,
    /// Pin it, or move it among the pinned threads.
    Pin {
        #[serde(default)]
        position: Position,
    },
    Unpin,
    /// Place it among the active threads, bringing it back from wherever it
    /// was.
    Arrange {
        position: Position,
    },
    Archive,
    Unarchive,
    MarkUnread,
    MarkRead,
    AutoSettle {
        mode: AutoSettle,
    },
    /// Undo: put back a lifecycle an earlier change returned.
    Restore {
        lifecycle: Lifecycle,
    },
}

/// Triage a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TriageThread {
    pub thread: ThreadId,
    #[serde(flatten)]
    pub action: TriageAction,
}

/// `thread.triage` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Triaged {
    /// The lifecycle before, for undoing.
    pub prior: Lifecycle,
    pub lifecycle: Lifecycle,
}

/// Give up a user-chosen title for one from the harness or the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RetitleThread {
    pub thread: ThreadId,
}

/// Result of renaming, refiling or retitling a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUpdated {
    pub title: String,
    pub project: Option<String>,
    /// The title before, for undoing.
    pub prior_title: String,
    /// The project before, for undoing.
    pub prior_project: Option<String>,
}
