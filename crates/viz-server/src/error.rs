//! Server failures before they are mapped onto a transport response.
//!
//! Callers branch on [`Error::kind`] or on a variant; only `crate::wire` knows
//! JSON-RPC codes.

use viz_core::{
    fork, lifecycle, placement, projects, requests,
    runs::{Busy, QueueRejection},
    tasks, threads,
};
use viz_protocol::{RunId, TaskId, ThreadId};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("no thread {0}")]
    NoThread(ThreadId),
    #[error("no run {run} in thread {thread}")]
    NoRun {
        thread: ThreadId,
        run: RunId,
    },
    #[error("no task {0}")]
    NoTask(TaskId),
    #[error("no project {0}")]
    NoProject(String),
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Queue(#[from] QueueRejection),
    #[error(transparent)]
    Busy(#[from] Busy),
    #[error(transparent)]
    Thread(#[from] threads::Rejection),
    #[error(transparent)]
    Placement(#[from] placement::Rejection),
    #[error(transparent)]
    Fork(#[from] fork::Rejection),
    #[error(transparent)]
    Triage(#[from] lifecycle::Rejection),
    #[error(transparent)]
    Task(#[from] tasks::Rejection),
    #[error(transparent)]
    Project(#[from] projects::Rejection),
    #[error(transparent)]
    Answer(#[from] requests::Rejection),
    /// Input that violates a documented constraint.
    #[error("{0}")]
    Invalid(String),
    /// A dependency is stopped, unreachable, or still working.
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Request(#[from] RequestError),
    /// A configured environment answered with an error.
    #[error("{message}")]
    Peer {
        kind: Kind,
        code: i32,
        message: String,
    },
    #[error("workspace database: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("workspace SQLite writer stopped")]
    WriterStopped,
    /// A stored value this version cannot interpret.
    #[error("corrupt workspace record: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Git(#[from] viz_git::Error),
    #[error(transparent)]
    GitHub(#[from] viz_github::Error),
    /// A harness operation failed.
    #[error(transparent)]
    Harness(#[from] viz_acp::Error),
}

/// Failures of the JSON-RPC exchange itself.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RequestError {
    #[error("{0}")]
    Malformed(String),
    #[error("{0}")]
    NotARequest(String),
    #[error("{0}")]
    UnknownMethod(String),
    #[error("call hello first")]
    HelloRequired,
}

/// What a caller can do about an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Invalid,
    NotFound,
    Unavailable,
    Internal,
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }

    pub(crate) fn corrupt(message: impl std::fmt::Display) -> Self {
        Self::Corrupt(message.to_string())
    }

    pub(crate) const fn kind(&self) -> Kind {
        match self {
            Self::NoThread(_)
            | Self::NoRun { .. }
            | Self::NoTask(_)
            | Self::NoProject(_)
            | Self::NotFound(_) => Kind::NotFound,
            Self::Queue(_)
            | Self::Busy(_)
            | Self::Thread(_)
            | Self::Task(_)
            | Self::Answer(_)
            | Self::Triage(_)
            | Self::Fork(_)
            | Self::Git(_)
            | Self::Project(_)
            | Self::Placement(_)
            | Self::Invalid(_)
            | Self::Request(_) => Kind::Invalid,
            Self::Unavailable(_) | Self::GitHub(_) => Kind::Unavailable,
            Self::Peer { kind, .. } => *kind,
            Self::Storage(_)
            | Self::WriterStopped
            | Self::Corrupt(_)
            | Self::Json(_)
            | Self::Io(_)
            | Self::Harness(_) => Kind::Internal,
        }
    }

    /// Whether repeating the same request cannot succeed, so an offer that got
    /// this answer was definitely refused.
    pub(crate) const fn rejected(&self) -> bool {
        matches!(self.kind(), Kind::Invalid | Kind::NotFound)
    }
}
