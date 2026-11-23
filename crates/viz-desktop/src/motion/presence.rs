//! Presence, directional view-switch, popover, and pane-slide transitions.

use rune::{Align, El, Side, SpringPair, Transition};
use rune_kit::{Placement, shell::PaneMotion};

use super::tokens::{
    GLIDE, OVERLAY_SCALE, PANE_FADE_TAIL, PANE_MAX_DURATION, PANE_MIN_DURATION, PANE_REDUCED_FADE,
    PANE_REF_WIDTH, QUICK_PAIR, REFLOW_PAIR, SETTLE_PAIR, SNAP_PAIR, STAGGER_BULK_LIMIT,
    STAGGER_MAX_ITEMS, STAGGER_STEP, TITLE_PAIR, TRAVEL_DRAWER, TRAVEL_MICRO, TRAVEL_SMALL,
};

/// Which way a pane or view stack moves when selection changes.
///
/// For threads, rows further down the sidebar sit below (`Down` moves content
/// up) and a delegated child sits to the right of its parent (`Into` moves
/// content left). The composer and Tasks Dock stay put; Reduce Motion turns
/// directional travel into a calm fade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Switch {
    /// No motion: a draft becoming its live thread, or switching environments.
    #[default]
    Cut,
    /// Directionless crossfade: drafts, or threads hidden by the active filter.
    Fade,
    /// Toward a thread listed further down: incoming rises from below.
    Down,
    /// Toward a thread listed further up: incoming drops from above.
    Up,
    /// Into a delegated child thread or deeper view-stack level: incoming
    /// enters from the right.
    Into,
    /// Back to a parent thread or previous view-stack level: incoming enters
    /// from the left.
    Out,
}

impl Switch {
    /// Unit vector pointing toward the edge from which the incoming view
    /// enters.
    const fn from(self) -> (f32, f32) {
        match self {
            Self::Cut | Self::Fade => (0.0, 0.0),
            Self::Down => (0.0, 1.0),
            Self::Up => (0.0, -1.0),
            Self::Into => (1.0, 0.0),
            Self::Out => (-1.0, 0.0),
        }
    }

    /// Incoming transcript page transition across `travel` points.
    pub fn enter(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(x * travel, y * travel, SETTLE_PAIR)
    }

    /// Outgoing transcript page transition for `El::child_exit`.
    pub fn exit(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(-x * travel, -y * travel, SETTLE_PAIR)
    }

    /// Incoming toolbar title transition, meant to pair with
    /// `enter_delay(TITLE_ENTER_DELAY)`.
    pub fn title_enter(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(x * travel, y * travel, TITLE_PAIR)
    }

    /// Fast outgoing toolbar title transition so old and new text do not
    /// collide.
    pub fn title_exit(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(-x * travel, -y * travel, TITLE_PAIR)
    }

    /// Crisp incoming view-stack transition (command palette, folder browser).
    pub fn view_enter(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(x * travel, y * travel, QUICK_PAIR)
    }

    /// Crisp outgoing view-stack transition for `El::child_exit`.
    pub fn view_exit(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.transition(-x * travel, -y * travel, QUICK_PAIR)
    }

    fn transition(self, x: f32, y: f32, springs: SpringPair) -> Transition {
        if self == Self::Cut {
            return Transition::none();
        }
        Transition::offset(x, y)
            .combined(Transition::opacity())
            .springs(springs)
    }
}

/// Opacity-only presence transition on `springs`.
pub fn fade(springs: SpringPair) -> Transition {
    Transition::opacity().springs(springs)
}

/// Popover, context menu, or floating card presence transition offset
/// vertically by `offset_y`.
pub fn popover(offset_y: f32) -> Transition {
    Transition::offset(0.0, offset_y)
        .combined(Transition::scale(OVERLAY_SCALE))
        .combined(Transition::opacity())
        .springs(SNAP_PAIR)
}

/// Anchors `content` to the element keyed `anchor_key` with viewport flipping,
/// unresolved-anchor suppression, and viz's standard popover spring transition.
pub fn anchored_popover(content: El, anchor_key: &str, placement: Placement, gap: f32) -> El {
    let (side, align, offset_y) = match placement {
        Placement::Top => (Side::Top, Align::Center, TRAVEL_MICRO),
        Placement::TopStart => (Side::Top, Align::Start, TRAVEL_MICRO),
        Placement::TopEnd => (Side::Top, Align::End, TRAVEL_MICRO),
        Placement::Bottom => (Side::Bottom, Align::Center, -TRAVEL_MICRO),
        Placement::BottomStart => (Side::Bottom, Align::Start, -TRAVEL_MICRO),
        Placement::BottomEnd => (Side::Bottom, Align::End, -TRAVEL_MICRO),
        Placement::Left => (Side::Left, Align::Center, 0.0),
        Placement::Right => (Side::Right, Align::Center, 0.0),
        Placement::RightStart => (Side::Right, Align::Start, 0.0),
    };
    let placed = if matches!(side, Side::Top | Side::Bottom) {
        content.anchored_fit(anchor_key, side, align, gap)
    } else {
        content.anchored(anchor_key, side, align, gap)
    };
    placed.transition(popover(offset_y))
}

/// Centered modal dialog or command palette panel presence transition.
pub fn dialog() -> Transition {
    Transition::offset(0.0, TRAVEL_SMALL)
        .combined(Transition::scale(OVERLAY_SCALE))
        .combined(Transition::opacity())
        .springs(SNAP_PAIR)
}

/// Modal backdrop scrim fade.
pub fn scrim() -> Transition {
    fade(SNAP_PAIR)
}

/// Tucked surface rising from behind the composer (Tasks Dock, approval,
/// question, queue).
pub fn tucked_deck() -> Transition {
    Transition::offset(0.0, TRAVEL_SMALL)
        .combined(Transition::opacity())
        .springs(SETTLE_PAIR)
}

/// Tucked `@`/`$`/`/` completion drawer rising from the composer's top edge.
pub fn tucked_menu() -> Transition {
    Transition::offset(0.0, TRAVEL_DRAWER)
        .combined(Transition::opacity())
        .springs(QUICK_PAIR)
}

/// Bounded upward slide and fade for toasts and undo notifications.
pub fn toast() -> Transition {
    Transition::offset(0.0, TRAVEL_SMALL)
        .combined(Transition::opacity())
        .springs(SETTLE_PAIR)
}

/// Presence transition for an entering list row or timeline card.
pub fn row_enter(offset_y: f32) -> Transition {
    Transition::offset(0.0, offset_y)
        .combined(Transition::opacity())
        .springs(REFLOW_PAIR)
}

/// Compact scale-and-fade entrance for attachment and filter chips.
pub fn chip_enter() -> Transition {
    Transition::scale(0.92)
        .combined(Transition::opacity())
        .springs(QUICK_PAIR)
}

/// Capped entrance stagger for small list insertions; returns `0.0` for bulk
/// updates.
pub fn stagger_delay(index: usize, total: usize) -> f32 {
    if total > STAGGER_BULK_LIMIT {
        return 0.0;
    }
    let rank = u16::try_from(index.min(STAGGER_MAX_ITEMS)).unwrap_or(u16::MAX);
    STAGGER_STEP * f32::from(rank)
}

/// Width-scaled `SplitView` motion for sidebars, diff panels, and inspectors.
pub fn pane_motion() -> PaneMotion {
    PaneMotion::new()
        .spring(GLIDE)
        .scale_with_width(PANE_REF_WIDTH, PANE_MIN_DURATION, PANE_MAX_DURATION)
        .fade_tail(PANE_FADE_TAIL)
        .reduced_motion_fade(PANE_REDUCED_FADE)
}

/// Locks text layout to its destination width while an adjacent pane opens or
/// closes.
pub fn stable_content(target: El) -> El {
    target.hold_landing_width()
}
