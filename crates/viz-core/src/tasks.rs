//! Durable delegation.
//!
//! A task is persisted before its offer is sent and stays persisted until its
//! result is delivered to the parent thread, so every step can be repeated
//! after a crash or a partition:
//!
//! - [`next`] reads the step to take from the persisted task alone.
//! - The shell carries the step out, then feeds what happened to [`decide`]
//!   together with the task reloaded under the write lease: it may have been
//!   cancelled while a peer was answering.
//!
//! A pending task can be cancelled locally. Once an offer may have been sent,
//! the receiver may already have persisted its receipt even if the response
//! was lost, so a cancellation retracts the offer: the receiver answers with
//! its receipt if it admitted the task, or records a tombstone that refuses
//! the offer if it is still under way. A receipt obtained while retracting
//! becomes a cancellation target. Failed transport calls decide nothing.
//!
//! Cancelling a run stops its whole [`Family`]: the tasks delegated from it,
//! the runs their results woke, and so on. The stop is recorded first, which
//! keeps the family's results from waking anyone, and is handled only once the
//! family has drained.
//!
//! The receiving thread records a wake in the same transaction as its entry
//! and prompt under the task and [`WakeSource`], so retries cannot wake twice.

use std::collections::{HashMap, HashSet};

use viz_protocol::{
    Run, RunId, RunKind, RunStatus, TaskId, TaskStatus, ThreadId,
    peer::{ReceiveTask, Received},
};

use crate::runs::is_open;

/// Where a task's admission and execution stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Recorded, never offered.
    Pending,
    /// The offer may have reached the receiver.
    Offered,
    /// Cancelled after an offer: the receiver has yet to say whether it
    /// admitted the task.
    Retracting,
    /// The receiver runs it.
    Accepted(Received),
    /// The receiver runs it and must stop it.
    Cancelling(Received),
    /// Over; the receipt remains if the task was ever admitted.
    Settled(Option<Received>),
}

impl Phase {
    /// The receiver's durable thread and run, once known.
    #[must_use]
    pub const fn receipt(self) -> Option<Received> {
        match self {
            Self::Pending | Self::Offered | Self::Retracting => None,
            Self::Accepted(receipt) | Self::Cancelling(receipt) => Some(receipt),
            Self::Settled(receipt) => receipt,
        }
    }

    /// Whether the offer may still be sent, so its payload must be kept.
    #[must_use]
    pub const fn offerable(self) -> bool {
        match self {
            Self::Pending | Self::Offered => true,
            Self::Retracting | Self::Accepted(_) | Self::Cancelling(_) | Self::Settled(_) => false,
        }
    }
}

/// How a result appears in the parent thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeMode {
    /// As an input that starts a run.
    Prompt,
    /// As a transcript entry only.
    EntryOnly,
}

/// Whether the parent has the result yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Owed(WakeMode),
    /// Delivered, waking the run if it prompted one.
    Delivered(Option<RunId>),
}

/// The persisted state decisions are made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub phase: Phase,
    pub status: TaskStatus,
    pub delivery: Delivery,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// The step a persisted task needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Record that the offer is about to leave.
    Offer,
    /// Send the offer, which may be a repetition.
    Send,
    /// Ask the receiver to give the offer back.
    Retract,
    /// Ask the receiver how its run is doing, or to stop it.
    Poll {
        receipt: Received,
        cancel: bool,
    },
    /// Give the parent thread the result.
    Deliver(WakeMode),
    Done,
}

/// Derive the next step.
#[must_use]
pub const fn next(task: &Task) -> Step {
    let mode = match task.delivery {
        Delivery::Owed(mode) => mode,
        Delivery::Delivered(_) => return Step::Done,
    };
    match task.phase {
        Phase::Pending => Step::Offer,
        Phase::Offered => Step::Send,
        Phase::Retracting => Step::Retract,
        Phase::Accepted(receipt) => Step::Poll {
            receipt,
            cancel: false,
        },
        Phase::Cancelling(receipt) => Step::Poll {
            receipt,
            cancel: true,
        },
        Phase::Settled(_) => Step::Deliver(mode),
    }
}

/// What happened to a task.
#[derive(Debug, Clone, Copy)]
pub enum Command<'a> {
    /// Its offer is about to be sent.
    Offer,
    /// The user or its parent cancelled it.
    Cancel,
    /// The family it belongs to was stopped.
    Suppress,
    /// The receiver admitted it, now or earlier.
    Admitted(Received),
    /// The receiver definitely refused the offer.
    Refused(&'a str),
    /// The receiver holds no receipt and will refuse the offer from now on.
    Retracted,
    /// The receiver reported on the run.
    Progressed(&'a Run),
    /// The receiver no longer has the run.
    Vanished,
    /// The parent thread has the result, and perhaps a run for it.
    Delivered(Option<RunId>),
}

/// The task after a command, or `None` when the command changes nothing.
#[must_use]
pub fn decide(task: &Task, command: Command<'_>) -> Option<Task> {
    let next = match command {
        Command::Offer => match task.phase {
            Phase::Pending => task.moved(Phase::Offered),
            Phase::Offered
            | Phase::Retracting
            | Phase::Accepted(_)
            | Phase::Cancelling(_)
            | Phase::Settled(_) => return None,
        },
        Command::Cancel => cancel(task),
        Command::Suppress => match task.delivery {
            Delivery::Owed(WakeMode::Prompt) => Task {
                delivery: Delivery::Owed(WakeMode::EntryOnly),
                ..cancel(task)
            },
            Delivery::Owed(WakeMode::EntryOnly) | Delivery::Delivered(_) => return None,
        },
        Command::Admitted(receipt) => match task.phase {
            Phase::Offered => task.moved(Phase::Accepted(receipt)),
            Phase::Retracting => task.moved(Phase::Cancelling(receipt)),
            Phase::Pending | Phase::Accepted(_) | Phase::Cancelling(_) | Phase::Settled(_) => {
                return None;
            }
        },
        Command::Refused(error) => match task.phase {
            Phase::Offered => task.settled(TaskStatus::Failed, None, Some(error)),
            Phase::Retracting => task.settled(TaskStatus::Cancelled, None, None),
            Phase::Pending | Phase::Accepted(_) | Phase::Cancelling(_) | Phase::Settled(_) => {
                return None;
            }
        },
        Command::Retracted => match task.phase {
            Phase::Retracting => task.settled(TaskStatus::Cancelled, None, None),
            Phase::Pending
            | Phase::Offered
            | Phase::Accepted(_)
            | Phase::Cancelling(_)
            | Phase::Settled(_) => return None,
        },
        Command::Progressed(run) => match task.phase {
            Phase::Accepted(_) | Phase::Cancelling(_) => progressed(task, run)?,
            Phase::Pending | Phase::Offered | Phase::Retracting | Phase::Settled(_) => return None,
        },
        Command::Vanished => match task.phase {
            Phase::Accepted(_) => task.settled(
                TaskStatus::Failed,
                None,
                Some("the assigned run no longer exists"),
            ),
            Phase::Cancelling(_) => task.settled(TaskStatus::Cancelled, None, None),
            Phase::Pending | Phase::Offered | Phase::Retracting | Phase::Settled(_) => return None,
        },
        Command::Delivered(wake) => Task {
            delivery: Delivery::Delivered(wake),
            ..task.clone()
        },
    };
    (next != *task).then_some(next)
}

fn cancel(task: &Task) -> Task {
    match task.phase {
        Phase::Pending => task.settled(TaskStatus::Cancelled, None, None),
        Phase::Offered => task.moved(Phase::Retracting),
        Phase::Accepted(receipt) => task.moved(Phase::Cancelling(receipt)),
        Phase::Retracting | Phase::Cancelling(_) | Phase::Settled(_) => task.clone(),
    }
}

fn progressed(task: &Task, run: &Run) -> Option<Task> {
    Some(match run.status {
        RunStatus::Queued => return None,
        RunStatus::Running => Task {
            status: TaskStatus::Running,
            ..task.clone()
        },
        RunStatus::Completed => task.settled(TaskStatus::Completed, run.answer.as_deref(), None),
        RunStatus::Failed => task.settled(
            TaskStatus::Failed,
            None,
            Some(run.error.as_deref().unwrap_or("run failed")),
        ),
        RunStatus::Interrupted => task.settled(
            TaskStatus::Failed,
            None,
            Some(run.error.as_deref().unwrap_or("run interrupted")),
        ),
        RunStatus::Cancelled => task.settled(TaskStatus::Cancelled, None, None),
    })
}

impl Task {
    fn moved(&self, phase: Phase) -> Self {
        Self {
            phase,
            ..self.clone()
        }
    }

    fn settled(&self, status: TaskStatus, result: Option<&str>, error: Option<&str>) -> Self {
        Self {
            phase: Phase::Settled(self.phase.receipt()),
            status,
            delivery: self.delivery,
            result: result.map(str::to_owned),
            error: error.map(str::to_owned),
        }
    }

    /// What the parent thread is told once the task settles.
    #[must_use]
    pub fn report(&self, id: &TaskId, title: &str, environment: &str) -> String {
        let location = self.phase.receipt().map_or_else(
            || environment.to_owned(),
            |receipt| format!("{environment} thread {}", receipt.thread),
        );
        let outcome = match self.status {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        };
        let mut text = format!("Task {id} ({title}) on {location} {outcome}.");
        if let Some(details) = self.result.as_deref().or(self.error.as_deref()) {
            text.push_str("\n\n");
            text.push_str(details);
        }
        text
    }
}

/// Whether a task has an outcome.
#[must_use]
pub const fn is_settled(status: TaskStatus) -> bool {
    match status {
        TaskStatus::Queued | TaskStatus::Running => false,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => true,
    }
}

/// A task's display title: the start of its prompt's first line.
#[must_use]
pub fn title(text: &str) -> String {
    text.trim()
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect()
}

/// Why a task cannot be delegated or admitted.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("unknown environment {0}")]
    UnknownEnvironment(String),
    #[error("task requires nonempty text")]
    EmptyText,
    #[error("task requestId must contain 1–256 bytes")]
    RequestId,
    #[error("the delegating run is no longer active")]
    StaleRun,
    #[error("the delegating run is being cancelled")]
    StoppingRun,
    #[error("task was cancelled before admission")]
    Retracted,
    #[error("target thread is not owned by this task parent")]
    ForeignThread,
    #[error("existing task thread cannot change its harness, project, model, or thinking")]
    RetargetedThread,
    #[error("an existing task thread cannot change its worktree")]
    MovedThread,
    #[error("task follow-up child does not match its receipt")]
    ForeignFollowUp,
}

/// The parent thread as a delegation finds it.
#[derive(Debug, Clone, Copy)]
pub struct Delegator {
    /// Its running run.
    pub active: Option<RunId>,
    /// That run's family is being stopped.
    pub stopping: bool,
}

/// Choose the run a new task belongs to. `turn` is the run an agent delegates
/// from; a client delegates from whichever run is active, if any.
///
/// # Errors
/// An agent cannot delegate from a run that ended, and no one can add to a
/// family that is being stopped.
pub fn open(parent: Delegator, turn: Option<RunId>) -> Result<Option<RunId>, Rejection> {
    if turn.is_some() && turn != parent.active {
        return Err(Rejection::StaleRun);
    }
    if parent.stopping {
        return Err(Rejection::StoppingRun);
    }
    Ok(parent.active)
}

/// A retry key accepted for a delegation.
///
/// # Errors
/// Rejects empty and oversized keys.
pub fn validate_request(text: &str, request_id: Option<&str>) -> Result<(), Rejection> {
    if text.trim().is_empty() {
        return Err(Rejection::EmptyText);
    }
    if request_id.is_some_and(|id| id.is_empty() || id.len() > 256) {
        return Err(Rejection::RequestId);
    }
    Ok(())
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

/// How the receiver answers an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The offer was admitted before.
    Replay(Received),
    Create,
}

/// Decide an offer. A receipt takes precedence over a tombstone, which refuses
/// late offers.
///
/// # Errors
/// Refuses a retracted task.
pub const fn admit(receipt: Option<Received>, retracted: bool) -> Result<Admission, Rejection> {
    match (receipt, retracted) {
        (Some(receipt), _) => Ok(Admission::Replay(receipt)),
        (None, true) => Err(Rejection::Retracted),
        (None, false) => Ok(Admission::Create),
    }
}

/// The thread an offer names, and the offer's demands on it.
#[derive(Debug, Clone, Copy)]
pub struct Reuse<'a> {
    pub offer: &'a ReceiveTask,
    pub parent: Option<ThreadId>,
    pub parent_environment: Option<&'a str>,
    pub harness: &'a str,
    pub project: Option<&'a str>,
}

/// Admit another task into a child thread, which keeps its harness and place.
///
/// # Errors
/// Only the parent that created the thread may reuse it, unchanged.
pub fn reuse(reuse: Reuse<'_>) -> Result<(), Rejection> {
    let ReceiveTask {
        parent,
        origin_environment,
        target,
        ..
    } = reuse.offer;
    if target.worktree.is_some() {
        return Err(Rejection::MovedThread);
    }
    if reuse.parent != Some(*parent) || reuse.parent_environment != Some(origin_environment) {
        return Err(Rejection::ForeignThread);
    }
    let differs = |wanted: &Option<String>, current: Option<&str>| {
        wanted.is_some() && wanted.as_deref() != current
    };
    if differs(&target.harness, Some(reuse.harness))
        || differs(&target.project, reuse.project)
        || target.model.is_some()
        || target.thinking.is_some()
    {
        return Err(Rejection::RetargetedThread);
    }
    Ok(())
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
pub fn descendants(root: RunId, edges: impl IntoIterator<Item = WakeEdge>) -> HashSet<RunId> {
    reachable(root, edges.into_iter().map(|edge| (edge.parent, edge.wake)))
}

/// Find receipt candidates by tracing the preceding prompt back through its
/// wakes.
#[must_use]
pub fn ancestors(original: RunId, edges: impl IntoIterator<Item = WakeEdge>) -> HashSet<RunId> {
    reachable(
        original,
        edges.into_iter().map(|edge| (edge.wake, edge.parent)),
    )
}

fn reachable(origin: RunId, edges: impl IntoIterator<Item = (RunId, RunId)>) -> HashSet<RunId> {
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

/// A task this environment admitted, kept for as long as its thread may
/// continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub environment: String,
    pub task: TaskId,
    pub run: RunId,
    /// The parent stopped the task's family.
    pub stopped: bool,
}

/// The tasks to report an agent's own continuation to, with the text to
/// report.
///
/// A completed continuation belongs to the prompt before it, and to that
/// prompt's task if the prompt is the task's run or was woken, directly or
/// not, by a result delivered to it. A prompt from anyone else breaks the
/// association.
#[must_use]
pub fn continued(
    continuation: &Run,
    preceding: Option<(RunId, RunStatus)>,
    edges: impl IntoIterator<Item = WakeEdge>,
    receipts: impl IntoIterator<Item = Receipt>,
) -> Option<(&str, Vec<Receipt>)> {
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
    let lineage = ancestors(prompt, edges);
    let owed = receipts
        .into_iter()
        .filter(|receipt| !receipt.stopped && lineage.contains(&receipt.run))
        .collect();
    Some((answer, owed))
}

/// How the parent's environment answers a reported continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUp {
    /// The task's own result has yet to be delivered; ask again.
    Later,
    /// Nothing to tell the parent.
    Discard,
    Wake(WakeMode),
}

/// Decide a continuation reported by `child`. It resumes the parent only when
/// the task's result did, and its family has not been stopped since.
///
/// # Errors
/// Refuses a report from a thread other than the task's.
pub fn follow_up(
    task: Option<&Task>,
    child: ThreadId,
    stopped: bool,
) -> Result<FollowUp, Rejection> {
    let Some(task) = task else {
        return Ok(FollowUp::Discard);
    };
    let Delivery::Delivered(wake) = task.delivery else {
        return Ok(FollowUp::Later);
    };
    if task.status != TaskStatus::Completed {
        return Ok(FollowUp::Discard);
    }
    if task.phase.receipt().map(|receipt| receipt.thread) != Some(child) {
        return Err(Rejection::ForeignFollowUp);
    }
    Ok(FollowUp::Wake(if wake.is_some() && !stopped {
        WakeMode::Prompt
    } else {
        WakeMode::EntryOnly
    }))
}
