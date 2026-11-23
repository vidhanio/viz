//! Thread state derived from committed runs, user metadata, and harness setup.

use viz_protocol::{RunStatus, ThreadStatus};

use crate::titles;

/// A patch can omit a value, clear it, or set it explicitly.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Patch<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

impl<T> Patch<T> {
    /// Apply a patch to an optional field.
    #[must_use]
    pub fn apply(self, previous: Option<T>) -> Option<T> {
        match self {
            Self::Keep => previous,
            Self::Clear => None,
            Self::Set(value) => Some(value),
        }
    }
}

/// A queued or running input takes precedence over setup and prior run
/// failures.
#[must_use]
pub const fn status(
    open_run: bool,
    setup_failed: bool,
    last_end: Option<RunStatus>,
) -> ThreadStatus {
    if open_run {
        ThreadStatus::Working
    } else if setup_failed || matches!(last_end, Some(RunStatus::Failed | RunStatus::Interrupted)) {
        ThreadStatus::Failed
    } else {
        ThreadStatus::Idle
    }
}

/// Why a thread's metadata cannot change.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("thread title must not be empty")]
    EmptyTitle,
    #[error("move the thread out of its worktree before changing its project")]
    InWorktree,
    #[error("only a project thread can move to a worktree")]
    NoProject,
    #[error("the thread moved concurrently")]
    Moved,
    #[error("requestId must contain 1 to 256 characters")]
    RequestId,
}

/// Validate an explicit title before recording it as user-owned.
///
/// # Errors
/// An explicitly supplied empty title is rejected.
pub fn validate_title(title: Option<&str>) -> Result<(), Rejection> {
    if title == Some("") {
        Err(Rejection::EmptyTitle)
    } else {
        Ok(())
    }
}

/// Validate a launch or send retry key.
///
/// # Errors
/// Rejects empty and oversized keys.
pub fn validate_request_id(request_id: Option<&str>) -> Result<(), Rejection> {
    if request_id.is_some_and(|id| id.is_empty() || id.chars().count() > 256) {
        Err(Rejection::RequestId)
    } else {
        Ok(())
    }
}

/// A thread's title, and whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Title {
    /// From the first prompt or the harness, which may replace it.
    Derived(String),
    User(String),
}

/// What the user can say about a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub title: Title,
    /// Where the thread is filed. Filing a thread does not move it: its
    /// directory changes only through a move (see [`crate::placement`]).
    pub project: Option<String>,
}

impl Metadata {
    /// The title as shown.
    #[must_use]
    pub fn title(&self) -> &str {
        match &self.title {
            Title::Derived(title) | Title::User(title) => title,
        }
    }
}

/// Changes to [`Metadata`].
#[derive(Debug, Default)]
pub struct Changes {
    pub title: Option<String>,
    pub project: Patch<String>,
}

/// Apply `changes` to a thread that may run in a linked worktree.
///
/// # Errors
/// Rejects an empty title, and filing a thread elsewhere than the project
/// whose worktree it runs in.
pub fn update(
    current: Metadata,
    in_worktree: bool,
    changes: Changes,
) -> Result<Metadata, Rejection> {
    validate_title(changes.title.as_deref())?;
    if in_worktree && !matches!(changes.project, Patch::Keep) {
        return Err(Rejection::InWorktree);
    }
    Ok(Metadata {
        title: changes.title.map_or(current.title, Title::User),
        project: changes.project.apply(current.project),
    })
}

/// Replace a title by the harness's latest, or else one from the latest
/// prompt.
#[must_use]
pub fn retitle(current: Metadata, harness: Option<&str>, prompt: Option<&str>) -> Metadata {
    let title = [harness, prompt]
        .into_iter()
        .flatten()
        .map(titles::from_prompt)
        .find(|title| !title.is_empty())
        .unwrap_or_else(|| current.title().to_owned());
    Metadata {
        title: Title::Derived(title),
        ..current
    }
}

/// Keep the original directory through repeated moves until a prompt uses the
/// handoff.
#[must_use]
pub fn handoff_from<'a>(
    has_history: bool,
    prior: Option<&'a str>,
    cwd: &'a str,
) -> Option<&'a str> {
    has_history.then_some(prior.unwrap_or(cwd))
}
