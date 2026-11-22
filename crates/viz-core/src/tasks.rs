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

use viz_protocol::{Run, RunId, RunKind, RunStatus, TaskStatus, ThreadId, peer::Received};

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
    outcome: Option<Run>,
    pub active: Vec<RunId>,
    pending_delivery: bool,
    cancelled_run: bool,
}

impl Family {
    /// Derive progress from the root and its wakes. A finished family reports
    /// the most recent wake's outcome unless cancellation won anywhere in the
    /// family.
    #[must_use]
    pub fn from_runs(
        mut outcome: Option<Run>,
        mut wakes: Vec<Run>,
        pending_delivery: bool,
    ) -> Self {
        let mut active: Vec<RunId> = outcome
            .iter()
            .filter(|run| is_open(run.status))
            .map(|run| run.id)
            .collect();
        wakes.sort_unstable_by_key(|run| std::cmp::Reverse(run.id));
        active.extend(
            wakes
                .iter()
                .filter(|run| is_open(run.status))
                .map(|run| run.id),
        );
        let cancelled_run = outcome
            .iter()
            .chain(&wakes)
            .any(|run| run.status == RunStatus::Cancelled);
        if active.is_empty()
            && !pending_delivery
            && !cancelled_run
            && let (Some(root), Some(wake)) = (&mut outcome, wakes.into_iter().next())
        {
            root.status = wake.status;
            root.answer = wake.answer;
            root.error = wake.error;
        }
        Self {
            outcome,
            active,
            pending_delivery,
            cancelled_run,
        }
    }

    /// Persist the stop intent before requesting cancellation from any
    /// descendants.
    #[must_use]
    pub const fn should_record_stop(&self, requested: bool, recorded: bool) -> bool {
        (requested || self.cancelled_run) && self.pending() && !recorded
    }

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

/// What a wake reports: the task's result, or a later continuation of the
/// child, identified by its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeSource {
    Result,
    Continuation(RunId),
}

impl WakeSource {
    /// The durable key of the wake among its task's wakes. Run IDs start at
    /// one.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        match self {
            Self::Result => 0,
            Self::Continuation(run) => run.0,
        }
    }
}

/// A wake is keyed by its task and source within the receiving thread.
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

/// The result of evaluating an admission request against its recorded key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Replay(Received),
    Cancelled,
    Create,
}

/// A receipt takes precedence over a tombstone, which prevents late offers.
#[must_use]
pub const fn admit(receipt: Option<Received>, retracted: bool) -> Admission {
    match (receipt, retracted) {
        (Some(receipt), _) => Admission::Replay(receipt),
        (None, true) => Admission::Cancelled,
        (None, false) => Admission::Create,
    }
}

/// A result delivered to a parent run can wake another local run in that
/// family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeEdge {
    pub parent: RunId,
    pub wake: RunId,
}

/// Compute all runs in the family, including the root and descendant
/// follow-ups.
#[must_use]
pub fn descendants(
    root: RunId,
    edges: impl IntoIterator<Item = WakeEdge>,
) -> std::collections::HashSet<RunId> {
    reachable(root, edges.into_iter().map(|edge| (edge.parent, edge.wake)))
}

/// Find receipt candidates by tracing the preceding prompt back through its
/// wakes.
#[must_use]
pub fn ancestors(
    original: RunId,
    edges: impl IntoIterator<Item = WakeEdge>,
) -> std::collections::HashSet<RunId> {
    reachable(
        original,
        edges.into_iter().map(|edge| (edge.wake, edge.parent)),
    )
}

fn reachable(
    origin: RunId,
    edges: impl IntoIterator<Item = (RunId, RunId)>,
) -> std::collections::HashSet<RunId> {
    use std::collections::{HashMap, HashSet};
    let mut adjacency: HashMap<RunId, Vec<RunId>> = HashMap::new();
    for (from, to) in edges {
        adjacency.entry(from).or_default().push(to);
    }
    let mut seen = HashSet::from([origin]);
    let mut pending = vec![origin];
    while let Some(from) = pending.pop() {
        for to in adjacency.remove(&from).into_iter().flatten() {
            if seen.insert(to) {
                pending.push(to);
            }
        }
    }
    seen
}

/// A completed continuation can inherit a completed prompt's receipt. A manual
/// prompt with no receipt or wake ancestry breaks the association.
#[must_use]
pub fn continuation_parent(
    continuation: &Run,
    preceding: Option<(RunId, RunStatus)>,
) -> Option<(RunId, &str)> {
    if continuation.kind != RunKind::Unprompted || continuation.status != RunStatus::Completed {
        return None;
    }
    let answer = continuation
        .answer
        .as_deref()
        .filter(|text| !text.trim_matches(' ').is_empty())?;
    let (prompt, RunStatus::Completed) = preceding? else {
        return None;
    };
    Some((prompt, answer))
}
