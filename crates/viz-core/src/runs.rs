//! A thread's runs: the queue, the single running slot, and how runs end.
//!
//! Decisions are applied while the shell holds the workspace's write lease. A
//! request ID replays the run it first created. A new input goes to the end of
//! the queue, or to its front when it steers the running one. The worker
//! claims the queue's head and persists its prompt in the same commit, before
//! the harness sees it; agent output outside a turn claims the same slot as a
//! continuation instead. Removing or stopping queued inputs cancels them
//! without transcript entries, and stopping the running one signals the
//! worker after the commit. A run ends once, with an end entry. A run found
//! running at startup was interrupted, and is never replayed.

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

/// The order that puts a queued input ahead of the others.
///
/// # Errors
/// The input must be queued.
pub fn promote(queue: &[RunId], run: RunId) -> Result<Vec<RunId>, QueueRejection> {
    if !queue.contains(&run) {
        return Err(QueueRejection::NotQueued);
    }
    let rest = queue.iter().copied().filter(|&other| other != run);
    Ok(std::iter::once(run).chain(rest).collect())
}

/// What an idle worker does with its thread's runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    Nothing,
    /// Take the queue's head.
    Start(RunId),
    /// End a run no worker is on: settling it failed. It holds the slot until
    /// then.
    Orphan(RunId),
}

/// Decide for a worker that is on no run itself.
#[must_use]
pub const fn claim(active: Option<RunId>, next: Option<RunId>) -> Claim {
    match (active, next) {
        (Some(run), _) => Claim::Orphan(run),
        (None, Some(run)) => Claim::Start(run),
        (None, None) => Claim::Nothing,
    }
}

/// What stopping a thread, or one of its runs, does.
#[derive(Debug, PartialEq, Eq)]
pub struct Cancellation {
    /// Queued inputs to cancel where they are.
    pub queued: Vec<RunId>,
    /// The running run, for its worker to stop.
    pub active: Option<RunId>,
}

/// Stop `wanted`, or everything.
#[must_use]
pub fn cancel(
    mut queued: Vec<RunId>,
    active: Option<RunId>,
    wanted: Option<RunId>,
) -> Cancellation {
    let applies = |run: RunId| wanted.is_none_or(|wanted| wanted == run);
    queued.retain(|&run| applies(run));
    Cancellation {
        queued,
        active: active.filter(|&run| applies(run)),
    }
}

/// Take one input out of the queue.
///
/// # Errors
/// The input must be queued.
pub fn remove(queue: &[RunId], run: RunId) -> Result<RunId, QueueRejection> {
    if queue.contains(&run) {
        Ok(run)
    } else {
        Err(QueueRejection::NotQueued)
    }
}

/// Whether the thread has no queued or running input, so its session can be
/// replaced.
///
/// # Errors
/// A busy thread must be stopped first.
pub const fn idle(queued: bool, active: Option<RunId>) -> Result<(), Busy> {
    if queued || active.is_some() {
        Err(Busy)
    } else {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("stop the thread's runs before moving it")]
pub struct Busy;

/// A terminal status is required wherever the shell persists a run end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndStatus {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl From<EndStatus> for RunStatus {
    fn from(status: EndStatus) -> Self {
        match status {
            EndStatus::Completed => Self::Completed,
            EndStatus::Failed => Self::Failed,
            EndStatus::Cancelled => Self::Cancelled,
            EndStatus::Interrupted => Self::Interrupted,
        }
    }
}

/// How the harness ended a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub answer: String,
    pub stop: Stop,
    /// The harness's name for the reason; a continuation has none.
    pub reason: Option<String>,
}

/// What the harness's stop reason means for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Finished,
    Cancelled,
    Refused,
}

/// Why the shell may have ended the turn itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    /// The server is shutting down.
    pub closing: bool,
    /// The run was stopped, or its thread deleted.
    pub stopped: bool,
    /// A steering input was waiting.
    pub steered: bool,
}

/// Settle a run at the end of its turn. The shell's own reasons win over
/// whatever the harness made of them, except that a turn which finished before
/// steering reached it counts as finished.
#[must_use]
pub fn settle(signals: Signals, turn: Result<TurnEnd, String>) -> Settlement {
    if signals.closing {
        return Settlement::interrupted();
    }
    if signals.stopped {
        return Settlement::stopped(EndStatus::Cancelled, "cancelled", None);
    }
    let TurnEnd {
        answer,
        stop,
        reason,
    } = match turn {
        Ok(end) => end,
        Err(error) => return Settlement::failed(error),
    };
    let (status, error) = match stop {
        Stop::Cancelled if signals.steered => {
            let mut settlement = Settlement::stopped(EndStatus::Cancelled, "steered", None);
            settlement.end.answer = Some(answer).filter(|answer| !answer.is_empty());
            return settlement;
        }
        Stop::Finished => (EndStatus::Completed, None),
        Stop::Cancelled => (EndStatus::Cancelled, None),
        Stop::Refused => (
            EndStatus::Failed,
            Some("ACP runner refused the prompt".into()),
        ),
    };
    Settlement {
        status,
        end: RunEnd {
            stop_reason: reason,
            answer: Some(answer),
            error,
        },
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

    /// End a run whose settlement could not be stored in time.
    #[must_use]
    pub fn orphaned() -> Self {
        Self::failed("the run's outcome could not be stored".into())
    }

    fn failed(error: String) -> Self {
        Self {
            status: EndStatus::Failed,
            end: RunEnd {
                stop_reason: Some("error".into()),
                error: Some(error),
                answer: None,
            },
        }
    }

    fn stopped(status: EndStatus, reason: &str, error: Option<&str>) -> Self {
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
