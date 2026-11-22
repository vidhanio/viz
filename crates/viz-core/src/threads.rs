//! Thread state derived from committed runs, user metadata, and harness setup.

use viz_protocol::{RunStatus, ThreadStatus};

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

/// Validate an explicit title before recording it as user-owned.
///
/// # Errors
/// An explicitly supplied empty title is rejected.
pub fn validate_title(title: Option<&str>) -> Result<(), EmptyTitle> {
    if title == Some("") {
        Err(EmptyTitle)
    } else {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("thread title must not be empty")]
pub struct EmptyTitle;

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
