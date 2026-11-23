//! Pure unified triage sidebar state types, snooze presets, optimistic
//! lifecycle transitions, and drag-drop target computation.

use std::collections::{BTreeMap, HashSet};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DragShelf {
    /// Pinned shelf (`TriageAction::Pin`).
    Pinned,
    /// Active triage queue (`TriageAction::Arrange` / `TriageAction::Unpin`).
    Active,
}

/// A rendered drop zone in window points, tracked only during a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DropZone {
    Viewport,
    Shelf(DragShelf),
    Row(DragShelf, ThreadId),
}

/// One insertion slot, shared by the preview marker and the committed move.
pub type DropTarget = (DragShelf, Option<ThreadId>);

/// Active pointer drag state for pinning and reordering sidebar thread cards.
#[derive(Debug, Clone)]
pub struct SidebarDrag {
    pub thread: ThreadId,
    pub source_shelf: DragShelf,
    pub target: Option<DropTarget>,
    pub dy: f32,
    pub active: bool,
    pub rects: BTreeMap<DropZone, [f32; 4]>,
}

impl SidebarDrag {
    /// Resolves the insertion slot from the pointer and measured shelf bounds.
    pub fn move_to(&mut self, dy: f32, point: [f32; 2], outline: &super::SidebarOutline<'_>) {
        self.dy = dy;
        self.active |= dy.abs() >= DRAG_THRESHOLD;
        self.target = self
            .active
            .then(|| self.target_at(point, outline))
            .flatten();
    }

    fn target_at(
        &self,
        [x, y]: [f32; 2],
        outline: &super::SidebarOutline<'_>,
    ) -> Option<DropTarget> {
        let contains = |zone| {
            self.rects
                .get(&zone)
                .is_some_and(|&[left, top, width, height]| {
                    x >= left && x <= left + width && y >= top && y <= top + height
                })
        };
        if !contains(DropZone::Viewport) {
            return None;
        }
        let (destination, rows) = [
            (DragShelf::Pinned, &outline.pinned),
            (DragShelf::Active, &outline.active),
        ]
        .into_iter()
        .find(|(shelf, _)| contains(DropZone::Shelf(*shelf)))?;
        let before = rows
            .iter()
            .map(|row| row.id)
            .filter(|&thread| thread != self.thread)
            .find(|&thread| {
                self.rects
                    .get(&DropZone::Row(destination, thread))
                    .is_some_and(|&[_, top, _, height]| y < height.mul_add(0.5, top))
            });
        Some((destination, before))
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
