//! Triage: which shelf a thread is on, what moves it there, and when it
//! settles or wakes by itself.
//!
//! A thread the user, or a due snooze, brought back is [`Standing::Kept`]: the
//! usual rules would otherwise settle it again at once. The user's next input
//! returns it to those rules.

use std::cmp::Reverse;

use viz_protocol::{
    Attention, AutoSettle, Lifecycle, Position, RunStatus, SettledBy, Shelf, Standing, ThreadId,
    TriageAction,
};

const DAY: i64 = 24 * 60 * 60 * 1000;

/// What a thread is doing, as far as triage cares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Activity {
    /// A run is queued or running.
    pub working: bool,
    /// The latest run, or preparing the harness, failed.
    pub failed: bool,
    /// The agent awaits the user.
    pub awaiting: Option<Awaiting>,
    /// Delegated tasks still under way.
    pub waiting_on: u32,
    /// The latest input or turn end, in Unix milliseconds.
    pub updated_at: i64,
    /// The user's latest input.
    pub last_input_at: Option<i64>,
    pub pull_request: Option<PullRequest>,
}

/// What the agent awaits from the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaiting {
    Approval,
    Question,
}

/// The pull request of a thread's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullRequest {
    /// Merged or closed.
    pub finished: bool,
    /// When it got that way.
    pub changed_at: i64,
}

/// The rules that settle threads by themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// After this many milliseconds without activity.
    pub idle: Option<i64>,
    /// When the pull request is merged or closed.
    pub finished_pull_request: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            idle: Some(7 * DAY),
            finished_pull_request: true,
        }
    }
}

/// Why a thread cannot be triaged as asked.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("the thread is working; stop it or snooze it instead")]
    Working,
    #[error("a snooze must end in the future")]
    Past,
    #[error("a settled thread cannot be pinned, arranged or snoozed")]
    Contradiction,
    #[error("thread {0} is not in that list")]
    NotListed(ThreadId),
}

/// The list to renumber for a change of position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum List {
    Pinned,
    Active,
}

/// The lifecycle after an action, and the place to make for it in a list (see
/// [`place`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Triage {
    pub lifecycle: Lifecycle,
    pub position: Option<(List, Position)>,
}

/// Apply an action by the user.
///
/// # Errors
/// See [`Rejection`].
pub fn triage(
    current: &Lifecycle,
    action: TriageAction,
    working: bool,
    now: i64,
) -> Result<Triage, Rejection> {
    let mut next = current.clone();
    let mut position = None;
    match action {
        TriageAction::Settle => {
            if working {
                return Err(Rejection::Working);
            }
            settle(&mut next, SettledBy::User, now);
        }
        TriageAction::Reopen => keep(&mut next),
        TriageAction::Snooze { until } => {
            if until <= now {
                return Err(Rejection::Past);
            }
            if matches!(next.standing, Standing::Settled { .. }) {
                next.standing = Standing::Kept;
            }
            next.snoozed_until = Some(until);
        }
        TriageAction::Wake => wake(&mut next),
        TriageAction::Pin { position: at } => {
            bring_back(&mut next);
            position = Some((List::Pinned, at));
        }
        TriageAction::Unpin => next.pin = None,
        TriageAction::Arrange { position: at } => {
            bring_back(&mut next);
            next.pin = None;
            position = Some((List::Active, at));
        }
        TriageAction::Archive => next.archived = true,
        TriageAction::Unarchive => {
            if next.archived && next.standing == Standing::Open {
                next.standing = Standing::Kept;
            }
            next.archived = false;
        }
        TriageAction::MarkUnread => next.unread = true,
        TriageAction::MarkRead => next.unread = false,
        TriageAction::AutoSettle { mode } => next.auto_settle = mode,
        TriageAction::Restore { lifecycle } => {
            if matches!(lifecycle.standing, Standing::Settled { .. })
                && (lifecycle.pin.is_some()
                    || lifecycle.order.is_some()
                    || lifecycle.snoozed_until.is_some())
            {
                return Err(Rejection::Contradiction);
            }
            next = lifecycle;
        }
    }
    Ok(Triage {
        lifecycle: next,
        position,
    })
}

const fn settle(lifecycle: &mut Lifecycle, by: SettledBy, at: i64) {
    lifecycle.standing = Standing::Settled { at, by };
    lifecycle.snoozed_until = None;
    lifecycle.pin = None;
    lifecycle.order = None;
    lifecycle.unread = false;
}

const fn keep(lifecycle: &mut Lifecycle) {
    if matches!(lifecycle.standing, Standing::Settled { .. }) {
        lifecycle.standing = Standing::Kept;
    }
}

const fn wake(lifecycle: &mut Lifecycle) {
    if lifecycle.snoozed_until.is_some() {
        lifecycle.snoozed_until = None;
        lifecycle.standing = Standing::Kept;
    }
}

const fn bring_back(lifecycle: &mut Lifecycle) {
    wake(lifecycle);
    keep(lifecycle);
    lifecycle.archived = false;
}

/// The positions that put `thread` at `position` in a list.
///
/// The list is given from top to bottom without the thread. `arranged` is how
/// many at the end of the list have a position already: those above keep
/// following activity unless the thread goes among or above them.
///
/// # Errors
/// The thread to go before must be in the list.
pub fn place(
    mut list: Vec<ThreadId>,
    arranged: usize,
    thread: ThreadId,
    position: Position,
) -> Result<Vec<(ThreadId, i64)>, Rejection> {
    let index = match position {
        Position::Top => 0,
        Position::Bottom => list.len(),
        Position::Before { thread: target } if target == thread => return Ok(Vec::new()),
        Position::Before { thread: target } => list
            .iter()
            .position(|&id| id == target)
            .ok_or(Rejection::NotListed(target))?,
    };
    let first = index.min(list.len() - arranged.min(list.len()));
    list.insert(index, thread);
    Ok(list.into_iter().skip(first).zip(0..).collect())
}

/// The user sent input.
#[must_use]
pub fn on_input(current: &Lifecycle) -> Lifecycle {
    Lifecycle {
        standing: Standing::Open,
        snoozed_until: None,
        archived: false,
        ..current.clone()
    }
}

/// The agent started on something without the user.
#[must_use]
pub fn on_agent_activity(current: &Lifecycle) -> Lifecycle {
    Lifecycle {
        standing: match current.standing {
            Standing::Settled { .. } => Standing::Open,
            standing @ (Standing::Open | Standing::Kept) => standing,
        },
        ..current.clone()
    }
}

/// A run ended. Only a failure ends a snooze: a snoozed thread may well be one
/// left to finish.
#[must_use]
pub fn on_run_end(current: &Lifecycle, status: RunStatus, watched: bool) -> Lifecycle {
    let mut next = current.clone();
    next.unread |= !watched;
    if matches!(status, RunStatus::Failed | RunStatus::Interrupted) {
        wake(&mut next);
    }
    next
}

/// The thread as time alone changes it, if it does: a due snooze ends, and an
/// idle or finished thread settles.
#[must_use]
pub fn on_time(
    current: &Lifecycle,
    activity: &Activity,
    policy: Policy,
    now: i64,
) -> Option<Lifecycle> {
    let mut next = current.clone();
    if let Some(until) = current.snoozed_until {
        if until > now {
            return None;
        }
        wake(&mut next);
        return Some(next);
    }
    let by = settle_due(current, activity, policy, now)?;
    let unread = next.unread;
    settle(&mut next, by, now);
    next.unread = unread;
    Some(next)
}

fn settle_due(
    current: &Lifecycle,
    activity: &Activity,
    policy: Policy,
    now: i64,
) -> Option<SettledBy> {
    if current.standing != Standing::Open
        || current.archived
        || current.auto_settle == AutoSettle::Never
        || activity.working
        || activity.waiting_on > 0
        || activity.awaiting.is_some()
    {
        return None;
    }
    let last_input = activity.last_input_at.unwrap_or(i64::MIN);
    if policy.finished_pull_request
        && activity
            .pull_request
            .is_some_and(|pr| pr.finished && pr.changed_at >= last_input)
    {
        return Some(SettledBy::PullRequest);
    }
    let idle = now.saturating_sub(activity.updated_at.max(last_input));
    policy
        .idle
        .is_some_and(|limit| idle >= limit)
        .then_some(SettledBy::Inactivity)
}

/// The shelf a thread is on. Pinned threads stay in place while they work.
#[must_use]
pub const fn shelf(lifecycle: &Lifecycle, activity: &Activity, now: i64) -> Shelf {
    if lifecycle.archived {
        return Shelf::Archived;
    }
    if matches!(lifecycle.standing, Standing::Settled { .. }) {
        return Shelf::Settled;
    }
    if let Some(until) = lifecycle.snoozed_until
        && until > now
    {
        return Shelf::Snoozed;
    }
    if lifecycle.pin.is_some() {
        return Shelf::Pinned;
    }
    if activity.working || activity.waiting_on > 0 || activity.awaiting.is_some() {
        return Shelf::Working;
    }
    Shelf::Active
}

/// What a thread wants from the user, most pressing first.
#[must_use]
pub const fn attention(lifecycle: &Lifecycle, activity: &Activity) -> Option<Attention> {
    match activity.awaiting {
        Some(Awaiting::Approval) => Some(Attention::Approval),
        Some(Awaiting::Question) => Some(Attention::Question),
        None if !lifecycle.unread => None,
        None if activity.failed && !activity.working => Some(Attention::Failure),
        None => Some(Attention::Unread),
    }
}

/// Sorts a thread list from top to bottom: by shelf, and within each by the
/// user's order, the next to wake, or the latest activity.
#[must_use]
pub fn sort_key(
    shelf: Shelf,
    lifecycle: &Lifecycle,
    activity: &Activity,
    thread: ThreadId,
) -> impl Ord + use<> {
    let latest = activity.last_input_at.unwrap_or(activity.updated_at);
    let (first, second) = match shelf {
        Shelf::Pinned => (lifecycle.pin.unwrap_or_default(), 0),
        Shelf::Working => (0, -latest),
        Shelf::Active => lifecycle
            .order
            .map_or((i64::MIN, -latest), |order| (order, 0)),
        Shelf::Snoozed => (lifecycle.snoozed_until.unwrap_or_default(), 0),
        Shelf::Settled => match lifecycle.standing {
            Standing::Settled { at, .. } => (-at, 0),
            Standing::Open | Standing::Kept => (0, 0),
        },
        Shelf::Archived => (-activity.updated_at, 0),
    };
    (shelf, first, second, Reverse(thread))
}
