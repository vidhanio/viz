//! Pure unified triage sidebar state types, snooze presets, optimistic
//! lifecycle transitions, and drag-drop target computation.

use std::collections::HashSet;

use viz_client::protocol::{
    Attention, Lifecycle, SettledBy, Shelf, Standing, ThreadId, ThreadStatus, ThreadSummary,
    TriageAction,
};

/// Shelf key for the collapsible Working section.
pub const SHELF_WORKING: &str = "working";
/// Shelf key for the collapsible Snoozed section.
pub const SHELF_SNOOZED: &str = "snoozed";
/// Shelf key for the bottom-anchored Settled drawer.
pub const SHELF_SETTLED: &str = "settled";
/// Number of settled threads revealed per page in the Settled drawer.
pub const SETTLED_PAGE_SIZE: usize = 15;
/// Pointer vertical movement in points before a card press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 6.0;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Active project filter restricting visible triage shelves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProjectFilter {
    /// Show threads from every project and projectless threads.
    #[default]
    All,
    /// Show only threads belonging to this project path.
    Project(String),
    /// Show only projectless (scratch) threads.
    NoProject,
}

impl ProjectFilter {
    /// Whether `project` passes this filter given `known_projects`.
    #[must_use]
    pub fn accepts(&self, project: Option<&str>, known_projects: &HashSet<&str>) -> bool {
        let resolved = project.filter(|path| known_projects.contains(path));
        match self {
            Self::All => true,
            Self::Project(path) => resolved == Some(path.as_str()),
            Self::NoProject => resolved.is_none(),
        }
    }
}

/// Submenu expanded inside a thread card's context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSubmenu {
    /// Snooze duration presets (`In 1 hour`, `In 3 hours`, etc.).
    Snooze,
    /// Auto-settle override (`Default` vs `Never auto-settle`).
    AutoSettle,
    /// Reassign project submenu.
    MoveToProject,
    /// Copy thread ID / branch / worktree path submenu.
    Copy,
}

/// Active inline thread title editor state inside Row 2 of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineRename {
    /// Thread being renamed.
    pub thread: ThreadId,
    /// Current draft title string.
    pub value: String,
}

/// Triage shelf participating in drag-and-drop pinning and reordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragShelf {
    /// Pinned shelf (`TriageAction::Pin`).
    Pinned,
    /// Active triage queue (`TriageAction::Arrange` / `TriageAction::Unpin`).
    Active,
}

/// Active pointer drag state for pinning and reordering sidebar thread cards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarDrag {
    /// Thread card being pressed or dragged.
    pub thread: ThreadId,
    /// Shelf the drag started from.
    pub source_shelf: DragShelf,
    /// Index in `source_shelf` at drag start.
    pub source_index: usize,
    /// Currently targeted destination shelf.
    pub target_shelf: DragShelf,
    /// Insertion index within `target_shelf` (0..=len).
    pub target_index: usize,
    /// Vertical pointer displacement from press origin in points.
    pub dy: f32,
    /// Card pitch (`height + gap`) measured when the drag activated.
    pub pitch: f32,
    /// True once vertical pointer travel crossed [`DRAG_THRESHOLD`].
    pub active: bool,
}

impl SidebarDrag {
    /// Updates target shelf and insertion slot from vertical displacement `dy`.
    pub fn move_to(&mut self, dy: f32, height: f32, pinned_len: usize, active_len: usize) {
        self.dy = dy;
        if !self.active {
            if dy.abs() < DRAG_THRESHOLD {
                return;
            }
            self.active = true;
            self.pitch = (height + 2.0).max(40.0);
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "sidebar card count fits well within isize"
        )]
        let steps = (dy / self.pitch).round().clamp(-10_000.0, 10_000.0) as isize;
        let combined_source = match self.source_shelf {
            DragShelf::Pinned => self.source_index,
            DragShelf::Active => pinned_len + self.source_index,
        };
        let total = pinned_len + active_len;
        let raw_target = combined_source.cast_signed().saturating_add(steps);
        if raw_target < 0 {
            self.target_shelf = DragShelf::Pinned;
            self.target_index = 0;
            return;
        }
        let target = usize::try_from(raw_target).unwrap_or_default().min(total);
        match self.source_shelf {
            DragShelf::Pinned => {
                if target <= pinned_len {
                    self.target_shelf = DragShelf::Pinned;
                    self.target_index = target.min(pinned_len);
                } else {
                    self.target_shelf = DragShelf::Active;
                    self.target_index = (target - pinned_len).min(active_len);
                }
            }
            DragShelf::Active => {
                if pinned_len == 0 && steps < 0 && combined_source.cast_signed() + steps < 0 {
                    self.target_shelf = DragShelf::Pinned;
                    self.target_index = 0;
                } else if pinned_len > 0 && target < pinned_len {
                    self.target_shelf = DragShelf::Pinned;
                    self.target_index = target;
                } else {
                    self.target_shelf = DragShelf::Active;
                    self.target_index = target.saturating_sub(pinned_len).min(active_len);
                }
            }
        }
    }
}

/// Snooze duration preset offered in the hover popover and context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozePreset {
    /// Wake 1 hour from now.
    OneHour,
    /// Wake 3 hours from now.
    ThreeHours,
    /// Wake tomorrow at 09:00 UTC.
    TomorrowMorning,
    /// Wake next Monday at 09:00 UTC.
    NextMonday,
}

impl SnoozePreset {
    /// All presets in menu display order.
    pub const ALL: [Self; 4] = [
        Self::OneHour,
        Self::ThreeHours,
        Self::TomorrowMorning,
        Self::NextMonday,
    ];

    /// Display label in menus and popovers.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::OneHour => "In 1 hour",
            Self::ThreeHours => "In 3 hours",
            Self::TomorrowMorning => "Tomorrow 9:00 AM",
            Self::NextMonday => "Next Monday",
        }
    }

    /// Resolves the wake timestamp in Unix milliseconds from `now_ms`.
    #[must_use]
    pub const fn until_ms(self, now_ms: i64) -> i64 {
        let now = if now_ms > 0 { now_ms } else { 0 };
        match self {
            Self::OneHour => now.saturating_add(HOUR_MS),
            Self::ThreeHours => now.saturating_add(3 * HOUR_MS),
            Self::TomorrowMorning => {
                let day = now.div_euclid(DAY_MS) + 1;
                day * DAY_MS + 9 * HOUR_MS
            }
            Self::NextMonday => {
                let day = now.div_euclid(DAY_MS);
                let weekday = (day + 3).rem_euclid(7);
                let ahead = 7 - weekday;
                (day + ahead) * DAY_MS + 9 * HOUR_MS
            }
        }
    }
}

/// Undo payload attached to a 5-second triage undo toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoTriage {
    /// Thread whose lifecycle can be restored.
    pub thread: ThreadId,
    /// Lifecycle snapshot before the triage mutation.
    pub prior: Lifecycle,
}

/// Computes the shelf for `thread` at `now`, matching `viz-core::lifecycle`.
#[must_use]
pub const fn effective_shelf(thread: &ThreadSummary, now: i64) -> Shelf {
    if thread.lifecycle.archived {
        return Shelf::Archived;
    }
    if matches!(thread.lifecycle.standing, Standing::Settled { .. }) {
        return Shelf::Settled;
    }
    if let Some(until) = thread.lifecycle.snoozed_until
        && (now == 0 || until > now)
    {
        return Shelf::Snoozed;
    }
    if thread.lifecycle.pin.is_some() {
        return Shelf::Pinned;
    }
    let working = matches!(thread.status, ThreadStatus::Working)
        || thread.waiting_on > 0
        || matches!(
            thread.attention,
            Some(Attention::Approval | Attention::Question)
        );
    if working {
        return Shelf::Working;
    }
    Shelf::Active
}

/// Recomputes `summary.shelf` and `summary.attention` after a lifecycle change.
pub const fn sync_summary_derived(summary: &mut ThreadSummary, now: i64) {
    summary.shelf = effective_shelf(summary, now);
    if !matches!(
        summary.attention,
        Some(Attention::Approval | Attention::Question)
    ) {
        summary.attention = if !summary.lifecycle.unread {
            None
        } else if matches!(summary.status, ThreadStatus::Failed) {
            Some(Attention::Failure)
        } else {
            Some(Attention::Unread)
        };
    }
}

/// Applies `action` optimistically to `summary` so shelves update immediately.
pub fn apply_optimistic_triage(summary: &mut ThreadSummary, action: &TriageAction, now: i64) {
    let lc = &mut summary.lifecycle;
    match action {
        TriageAction::Settle => {
            lc.standing = Standing::Settled {
                at: now,
                by: SettledBy::User,
            };
            lc.snoozed_until = None;
            lc.pin = None;
            lc.order = None;
            lc.unread = false;
        }
        TriageAction::Reopen => {
            if matches!(lc.standing, Standing::Settled { .. }) {
                lc.standing = Standing::Kept;
            }
        }
        TriageAction::Snooze { until } => {
            if matches!(lc.standing, Standing::Settled { .. }) {
                lc.standing = Standing::Kept;
            }
            lc.snoozed_until = Some(*until);
        }
        TriageAction::Wake => {
            if lc.snoozed_until.is_some() {
                lc.snoozed_until = None;
                lc.standing = Standing::Kept;
            }
        }
        TriageAction::Pin { .. } => {
            if lc.snoozed_until.is_some() {
                lc.snoozed_until = None;
                lc.standing = Standing::Kept;
            } else if matches!(lc.standing, Standing::Settled { .. }) {
                lc.standing = Standing::Kept;
            }
            lc.archived = false;
            if lc.pin.is_none() {
                lc.pin = Some(-1);
            }
        }
        TriageAction::Unpin => {
            lc.pin = None;
        }
        TriageAction::Arrange { .. } => {
            if lc.snoozed_until.is_some() {
                lc.snoozed_until = None;
                lc.standing = Standing::Kept;
            } else if matches!(lc.standing, Standing::Settled { .. }) {
                lc.standing = Standing::Kept;
            }
            lc.archived = false;
            lc.pin = None;
        }
        TriageAction::Archive => {
            lc.archived = true;
        }
        TriageAction::Unarchive => {
            if lc.archived && lc.standing == Standing::Open {
                lc.standing = Standing::Kept;
            }
            lc.archived = false;
        }
        TriageAction::MarkUnread => lc.unread = true,
        TriageAction::MarkRead => lc.unread = false,
        TriageAction::AutoSettle { mode } => lc.auto_settle = *mode,
        TriageAction::Restore { lifecycle } => *lc = lifecycle.clone(),
    }
    sync_summary_derived(summary, now);
}

/// Formats a compact wake time label (`in 45m`, `in 2h`, `Tomorrow`, `in 3d`)
/// for snoozed cards.
#[must_use]
pub fn format_wake_label(until_ms: i64, now_ms: i64) -> String {
    let remaining = until_ms.saturating_sub(now_ms).max(0);
    if remaining < HOUR_MS {
        let mins = (remaining / MINUTE_MS).max(1);
        format!("in {mins}m")
    } else if remaining < DAY_MS {
        let hours = (remaining + HOUR_MS / 2) / HOUR_MS;
        format!("in {hours}h")
    } else if remaining < 2 * DAY_MS {
        "Tomorrow".to_owned()
    } else {
        let days = (remaining + DAY_MS / 2) / DAY_MS;
        format!("in {days}d")
    }
}
