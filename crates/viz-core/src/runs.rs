//! Run queue transitions are applied while the shell holds a workspace write
//! lease.
//!
//! Request IDs return the original run before evaluating placement or
//! attachments. A new input goes at the end, or in front when steering an
//! active run. Promoting an existing queued input preserves its identity and
//! payload. The worker claims the queue head and persists its prompt in the
//! same commit before calling ACP. Idle ACP output starts a running
//! continuation with a continued entry instead. Dispatch and continuations
//! share the single running slot. Removing or stopping queued inputs cancels
//! them without transcript entries. Stopping a running input signals the worker
//! after commit. Settlement appends an end entry once and may
//! enqueue a delegated continuation answer. On restart a running input is
//! settled as interrupted, with no replay. If settlement itself fails,
//! abandoning it frees the slot through a bare failure update.

use viz_protocol::{RunEnd, RunId, RunStatus, SendMode};

/// Prompt dispatch placement within the queued runs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    #[default]
    End,
    Front,
}

impl Place {
    /// Compute a position relative to the remaining queue, if any.
    #[must_use]
    pub const fn position(self, bounds: Option<(i64, i64)>) -> Option<i64> {
        match (self, bounds) {
            (_, None) => Some(0),
            (Self::Front, Some((first, _))) => first.checked_sub(1),
            (Self::End, Some((_, last))) => last.checked_add(1),
        }
    }
}

/// The dispatch information needed to accept a new input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmissionState {
    pub previous: Option<RunId>,
    pub active: Option<RunId>,
}

/// A retry returns the recorded ID. A new input may steer the active run after
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submission {
    Replay(RunId),
    Enqueue { place: Place, steer: Option<RunId> },
}

/// Decide placement and the worker effect for a send.
#[must_use]
pub fn submit(state: SubmissionState, mode: Option<SendMode>) -> Submission {
    if let Some(run) = state.previous {
        return Submission::Replay(run);
    }
    let steer = state.active.filter(|_| mode == Some(SendMode::Steer));
    let place = if steer.is_some() {
        Place::Front
    } else {
        Place::End
    };
    Submission::Enqueue { place, steer }
}

/// A queue move independent of the edit to the prompt text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    #[default]
    Keep,
    End,
    Before(RunId),
}

/// Rejected queue commands carry the reason the caller can act on.
#[derive(Debug, thiserror::Error)]
pub enum QueueRejection {
    #[error("run is not queued")]
    NotQueued,
    #[error("target run is not queued")]
    TargetNotQueued,
}

/// Return the new queue order when an edit moves an input.
///
/// # Errors
/// Returns an error when the input or its requested predecessor is outside the
/// queue.
pub fn reorder(
    mut queue: Vec<RunId>,
    run: RunId,
    placement: Move,
) -> Result<Option<Vec<RunId>>, QueueRejection> {
    let original = queue
        .iter()
        .position(|&id| id == run)
        .ok_or(QueueRejection::NotQueued)?;
    let before = match placement {
        Move::Keep => return Ok(None),
        Move::Before(target) if target == run => return Ok(None),
        Move::Before(target) => Some(target),
        Move::End => None,
    };
    queue.remove(original);
    let index = match before {
        Some(target) => queue
            .iter()
            .position(|&id| id == target)
            .ok_or(QueueRejection::TargetNotQueued)?,
        None => queue.len(),
    };
    queue.insert(index, run);
    Ok(Some(queue))
}

/// Select the queue head only if the thread has a free running slot.
#[must_use]
pub const fn claim(active: Option<RunId>, next: Option<RunId>) -> Option<RunId> {
    if active.is_none() { next } else { None }
}

/// Select the active run to signal after queued inputs are cancelled.
#[must_use]
pub fn cancel_active(active: Option<RunId>, wanted: Option<RunId>) -> Option<RunId> {
    active.filter(|active| wanted.is_none_or(|wanted| wanted == *active))
}

/// A terminal status is required wherever the shell persists a run end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndStatus {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl EndStatus {
    /// Use the same names in SQLite and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

/// A terminal state and the corresponding transcript entry.
#[derive(Debug)]
pub struct Settlement {
    pub status: EndStatus,
    pub end: RunEnd,
}

impl Settlement {
    /// Recover an in-flight run without resending the prompt.
    #[must_use]
    pub fn interrupted() -> Self {
        Self::stopped(
            EndStatus::Interrupted,
            "interrupted",
            Some("server was interrupted during the ACP turn; the prompt was not resent"),
        )
    }

    /// A failed dispatch or prompt with no answer.
    #[must_use]
    pub fn failed(error: String) -> Self {
        Self {
            status: EndStatus::Failed,
            end: RunEnd {
                stop_reason: Some("error".into()),
                error: Some(error),
                answer: None,
            },
        }
    }

    /// Record why an active turn stopped.
    #[must_use]
    pub fn stopped(status: EndStatus, reason: &str, error: Option<&str>) -> Self {
        Self {
            status,
            end: RunEnd {
                stop_reason: Some(reason.into()),
                error: error.map(str::to_owned),
                answer: None,
            },
        }
    }
}

/// Queued and running runs are the two unsettled states.
#[must_use]
pub const fn is_open(status: RunStatus) -> bool {
    match status {
        RunStatus::Queued | RunStatus::Running => true,
        RunStatus::Completed
        | RunStatus::Failed
        | RunStatus::Cancelled
        | RunStatus::Interrupted => false,
    }
}
