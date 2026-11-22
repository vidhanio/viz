//! Durable delegation transitions.
//!
//! A pending offer can be cancelled locally. Once an offer is sent, the
//! receiver may already have persisted its receipt, even if the response was
//! lost. A cancellation therefore retracts the offer. The receiver returns its
//! receipt if admitted or records a tombstone if absent. Receipts obtained
//! while retracting become cancellation targets. Failed transport calls leave
//! the phase intact for retry. Accepted and cancelling tasks settle using the
//! receiver's reported run outcome.
//!
//! Cancellation follows the root's task family through descendant wakes,
//! including follow-ups. A durable stop request suppresses new prompt wakes
//! from that family and is handled only once the family drains. A terminal task
//! stores its outcome before delivery. The receiving thread records each (task,
//! sequence) wake in the same transaction as its entry and optional prompt, so
//! retries cannot wake twice.

use viz_protocol::{Run, RunId, RunKind, RunStatus, TaskStatus, ThreadId};

use crate::runs::is_open;

/// The durable admission and execution lifecycle of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Pending,
    Offered,
    Retracting,
    Accepted,
    Cancelling,
    Settled,
}

/// A stored phase must be one of the six protocol states.
#[derive(Debug, thiserror::Error)]
#[error("invalid persisted task phase {0}")]
pub struct InvalidPhase(String);

impl std::str::FromStr for Phase {
    type Err = InvalidPhase;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "offered" => Ok(Self::Offered),
            "retracting" => Ok(Self::Retracting),
            "accepted" => Ok(Self::Accepted),
            "cancelling" => Ok(Self::Cancelling),
            "settled" => Ok(Self::Settled),
            _ => Err(InvalidPhase(value.to_owned())),
        }
    }
}

impl Phase {
    /// Use the same spelling as the durable phase column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Offered => "offered",
            Self::Retracting => "retracting",
            Self::Accepted => "accepted",
            Self::Cancelling => "cancelling",
            Self::Settled => "settled",
        }
    }
}

/// Applying a phase command yields either a phase update or a terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Keep,
    Move(Phase),
    Settle(TaskStatus),
}

/// Commands derived from user cancellation, admission work, and peer replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Offer,
    Cancel,
    Receipt,
    Rejected,
    Retracted,
}

/// Apply a command to the current durable phase, reloaded after any peer call.
#[must_use]
pub const fn decide(phase: Phase, command: Command) -> Transition {
    use Command::{Cancel, Offer, Receipt, Rejected, Retracted};
    use Phase::{Accepted, Cancelling, Offered, Pending, Retracting, Settled};
    match (phase, command) {
        (Pending, Offer) => Transition::Move(Offered),
        (Offered, Cancel) => Transition::Move(Retracting),
        (Accepted, Cancel) | (Retracting, Receipt) => Transition::Move(Cancelling),
        (Offered, Receipt) => Transition::Move(Accepted),
        (Offered, Rejected) => Transition::Settle(TaskStatus::Failed),
        (Pending, Cancel) | (Retracting, Rejected | Retracted) => {
            Transition::Settle(TaskStatus::Cancelled)
        }
        (Pending, Receipt | Rejected | Retracted)
        | (Offered, Offer | Retracted)
        | (Retracting, Offer | Cancel)
        | (Accepted, Offer | Receipt | Rejected | Retracted)
        | (Cancelling | Settled, _) => Transition::Keep,
    }
}

/// Whether a durable result should resume the parent or just appear in its
/// transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMode {
    Prompt,
    EntryOnly,
}

/// Follow-ups resume only when the original result prompted and the family is
/// open.
#[must_use]
pub const fn follow_up_mode(original_wake: Option<RunId>, stopped: bool) -> WakeMode {
    if original_wake.is_some() && !stopped {
        WakeMode::Prompt
    } else {
        WakeMode::EntryOnly
    }
}

/// A root run and the delivery obligations for its descendants.
#[derive(Debug)]
pub struct Family {
    pub outcome: Option<Run>,
    pub active: Vec<RunId>,
    pub pending_delivery: bool,
    pub cancelled_run: bool,
}

impl Family {
    /// Stop is complete only after the running family and outstanding
    /// deliveries drain.
    #[must_use]
    pub const fn pending(&self) -> bool {
        !self.active.is_empty() || self.pending_delivery
    }

    /// Report running until all replies are delivered; cancellation wins once
    /// drained.
    #[must_use]
    pub fn progress(self, thread: ThreadId, run: RunId, stopped: bool) -> Option<Run> {
        if self.pending() {
            Some(
                self.outcome
                    .filter(|outcome| is_open(outcome.status))
                    .unwrap_or(Run {
                        id: run,
                        thread,
                        kind: RunKind::Prompt,
                        status: RunStatus::Running,
                        answer: None,
                        error: None,
                    }),
            )
        } else {
            self.outcome.map(|mut outcome| {
                if stopped || self.cancelled_run {
                    outcome.status = RunStatus::Cancelled;
                    outcome.answer = None;
                    outcome.error = None;
                }
                outcome
            })
        }
    }
}

/// The action to take when the peer reports the assigned run's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress<'a> {
    Wait,
    Started,
    Settled {
        status: TaskStatus,
        answer: Option<&'a str>,
        error: Option<&'a str>,
    },
}

/// Fold a peer run update into the task's presentation and terminal outcome.
#[must_use]
pub fn record_progress(previous: TaskStatus, run: &Run) -> Progress<'_> {
    match run.status {
        RunStatus::Running if previous == TaskStatus::Queued => Progress::Started,
        RunStatus::Queued | RunStatus::Running => Progress::Wait,
        RunStatus::Completed => Progress::Settled {
            status: TaskStatus::Completed,
            answer: run.answer.as_deref(),
            error: None,
        },
        RunStatus::Failed | RunStatus::Interrupted => Progress::Settled {
            status: TaskStatus::Failed,
            answer: None,
            error: run.error.as_deref().or_else(|| {
                Some(if run.status == RunStatus::Interrupted {
                    "run interrupted"
                } else {
                    "run failed"
                })
            }),
        },
        RunStatus::Cancelled => Progress::Settled {
            status: TaskStatus::Cancelled,
            answer: None,
            error: None,
        },
    }
}

/// Task status names are shared by SQLite and the wire format.
#[must_use]
pub const fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// A wake is keyed by its task and sequence within the receiving thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Replay(Option<RunId>),
    MissingThread,
    Append(WakeMode),
}

/// Decide whether a wake may create a transcript entry and optional run.
#[must_use]
pub const fn wake(previous: Option<Option<RunId>>, exists: bool, mode: WakeMode) -> Wake {
    match (previous, exists) {
        (Some(run), _) => Wake::Replay(run),
        (None, false) => Wake::MissingThread,
        (None, true) => Wake::Append(mode),
    }
}

/// Admission and retraction use the same durable receiver key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipt {
    pub thread: ThreadId,
    pub run: RunId,
}

/// The result of evaluating an admission request against its recorded key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Replay(Receipt),
    Cancelled,
    Create,
}

/// A receipt takes precedence over a tombstone, which prevents late offers.
#[must_use]
pub const fn admit(receipt: Option<Receipt>, retracted: bool) -> Admission {
    match (receipt, retracted) {
        (Some(receipt), _) => Admission::Replay(receipt),
        (None, true) => Admission::Cancelled,
        (None, false) => Admission::Create,
    }
}
