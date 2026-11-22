//! Motion tokens and reusable primitives for `viz-desktop`.
//!
//! Every spring is critically damped (`bounce = 0.0`) so utility motion stays
//! calm and preserves instantaneous position and velocity on mid-flight
//! interruption. Elevation during motion (overlays, popovers, lifted drag
//! cards) uses opaque base16 fills and `1px` hairlines — never drop shadows,
//! blur, or glow.
//!
//! Surfaces map onto these primitives directly:
//! - Presence enter/exit: [`fade`], [`popover`], [`dialog`], [`scrim`],
//!   [`tucked_deck`], [`tucked_menu`], [`toast`], [`row_enter`],
//!   [`chip_enter`], [`stagger_delay`], [`Switch`].
//! - List reorder / FLIP: [`reflow`], [`reflow_container`].
//! - Height disclosure with scroll anchoring: [`fold_container`], [`fold_row`],
//!   [`toggle_anchored`], [`chevron`].
//! - In-place icon crossfade / morph: [`icon_morph`].
//! - Number roll for counters and diff stats: [`rolling_count`],
//!   [`rolling_diff_stat`].
//! - Live row shimmer: [`shimmer_band`].
//! - Shared-element move: [`shared_pill`], [`shared_surface`], [`launch_from`].
//! - Panel slide: [`pane_motion`], [`slide_panel`], [`stable_content`].
//! - Drag settle: [`DragOrder`], [`drag_row`].

use std::f32::consts::FRAC_PI_2;

use rune::{
    Anim, Color, Drag, Easing, Edge, El, GrowthSpec, Icon, LoopSpec, Phase, Spring, SpringPair,
    TextStyle, Transition, Tx, el, icon, row,
};
use rune_kit::{Kit, RollingText, shell::PaneMotion};

use crate::theme::{HAIRLINE, S6, Theme};

/// Fast critically damped spring for hover entry, rapid exits, and title exits.
pub const INSTANT: Spring = Spring::new(0.10, 0.0);
/// Calm pointer-leave ink and wash fade across the window root.
pub const HOVER_OUT: Spring = Spring::new(0.12, 0.0);
/// Crisp control and micro-overlay spring (icon morphs, rail pill, tucked
/// menus).
pub const QUICK: Spring = Spring::new(0.14, 0.0);
/// Standard overlay, chevron, and rolling-counter spring.
pub const SNAP: Spring = Spring::new(0.20, 0.0);
/// Calm structural settle spring (height disclosures, thread switches, docks,
/// toasts).
pub const SETTLE: Spring = Spring::new(0.24, 0.0);
/// List reorder (FLIP), sibling step-aside, and drag-drop settle spring.
pub const REFLOW: Spring = Spring::new(0.28, 0.0);
/// Large sliding panel and hero-to-thread dock spring.
pub const GLIDE: Spring = Spring::new(0.28, 0.0);

/// Asymmetric enter/exit pair for compact menus, icon morphs, and tucked
/// completion drawers.
pub const QUICK_PAIR: SpringPair = SpringPair::new(QUICK, INSTANT);
/// Asymmetric enter/exit pair for dialogs, popovers, command palette, and
/// details cards.
pub const SNAP_PAIR: SpringPair = SpringPair::new(SNAP, QUICK);
/// Asymmetric enter/exit pair for the toolbar thread title so outgoing ink
/// clears first.
pub const TITLE_PAIR: SpringPair = SpringPair::new(SNAP, INSTANT);
/// Asymmetric enter/exit pair for transcript switches, Tasks Dock, banners, and
/// toasts.
pub const SETTLE_PAIR: SpringPair = SpringPair::new(SETTLE, Spring::new(0.18, 0.0));
/// Asymmetric enter/exit pair for entering/exiting list rows.
pub const REFLOW_PAIR: SpringPair = SpringPair::new(REFLOW, SNAP);
/// Asymmetric enter/exit pair for edge-sliding panels.
pub const GLIDE_PAIR: SpringPair = SpringPair::new(GLIDE, Spring::new(0.22, 0.0));

/// Micro travel distance for popovers, menus, and tooltips (points).
pub const TRAVEL_MICRO: f32 = 4.0;
/// Short upward emergence distance for tucked `@`/`$`/`/` completion menus
/// (points).
pub const TRAVEL_DRAWER: f32 = 6.0;
/// Compact vertical travel for toasts, Tasks Dock, composer banners, dialogs,
/// and rows (points).
pub const TRAVEL_SMALL: f32 = 12.0;
/// Toolbar title travel distance inside its `TOOLBAR_HEIGHT` clip (points).
pub const TRAVEL_TITLE: f32 = 24.0;
/// Transcript page travel distance on directional thread switches (points).
pub const TRAVEL_PANE: f32 = 40.0;

/// Tactile press scale for buttons (`El::press_scale`; auto-disabled under
/// Reduce Motion).
pub const PRESS_SCALE: f32 = 0.97;
/// Subtle enter/exit scale for dialogs, palettes, and popovers.
pub const OVERLAY_SCALE: f32 = 0.97;
/// In-place icon crossfade scale inside `El::dissolve` (above Rune's `0.5`
/// small-scale floor).
pub const MORPH_SCALE: f32 = 0.65;
/// Soft edge feather painted on the moving boundary of `fold_glide` (drops to
/// `0` at rest).
pub const FOLD_FEATHER: f32 = 12.0;

/// Delay before a new toolbar title enters so the outgoing title clears without
/// overlapping.
pub const TITLE_ENTER_DELAY: f32 = 0.08;
/// Delay before showing a thread loading caption so fast local loads never
/// flash a spinner.
pub const LOADING_DELAY: f32 = 0.40;
/// Per-item stagger interval for small list additions (seconds).
pub const STAGGER_STEP: f32 = 0.025;
/// Maximum staggered item rank (`150ms` total cap).
pub const STAGGER_MAX_ITEMS: usize = 6;
/// Row counts above this threshold skip per-row stagger and enter together.
pub const STAGGER_BULK_LIMIT: usize = 12;

/// Linear sweep period for live thinking and running tool shimmer bands
/// (seconds).
pub const SHIMMER_PERIOD: f64 = 1.8;
/// Fractional width of the swept shimmer beam across its host surface.
const SHIMMER_BEAM_FRAC: f32 = 0.45;

/// Reference width for `PaneMotion::scale_with_width` (points).
const PANE_REF_WIDTH: f32 = 360.0;
/// Shortest width-scaled pane spring duration (seconds).
const PANE_MIN_DURATION: f32 = 0.18;
/// Longest width-scaled pane spring duration (seconds).
const PANE_MAX_DURATION: f32 = 0.30;
/// Trailing closed fraction across which pane contents fade.
const PANE_FADE_TAIL: f32 = 0.15;
/// Content fade duration for panes under Reduce Motion (seconds).
const PANE_REDUCED_FADE: f32 = 0.12;

/// Pointer travel in points before a press on a reorderable row becomes a drag.
const DRAG_SLOP: f32 = 4.0;

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

/// Applies viz's standard tactile press scale (`0.97`, auto-disabled under
/// Reduce Motion).
pub fn pressable(target: El) -> El {
    target.press_scale(PRESS_SCALE)
}

/// Attaches zero-bounce FLIP layout reflow motion to a keyed list row.
pub fn reflow(target: El) -> El {
    target.motion(REFLOW)
}

/// Configures a list container so exiting rows dissolve cleanly while siblings
/// close the gap.
pub fn reflow_container(target: El) -> El {
    target.motion(REFLOW).child_exit(fade(REFLOW_PAIR))
}

/// Configures a parent column to host smoothly folding disclosure bodies.
///
/// Every sibling inside a `fold_container` must carry an explicit `.key(...)`
/// so inserting or removing a conditional fold body does not shift positional
/// identity and ghost neighbouring siblings.
pub fn fold_container(target: El) -> El {
    target.growth(GrowthSpec::default())
}

/// Configures a collapsible body for velocity-preserving feathered height
/// gliding.
pub fn fold_row(target: El) -> El {
    target.fold_glide(SETTLE).fold_feather(FOLD_FEATHER)
}

/// Pins a disclosure's header key under the pointer across its open/close
/// commit.
pub fn toggle_anchored(header_key: impl Into<String>, tx: &mut Tx) {
    tx.keep_on_screen(header_key);
}

/// Rotating disclosure chevron that opts back into animation inside
/// `instant_changes` subtrees.
pub fn chevron(glyph: &'static Icon, size: f32, open: bool, open_radians: f32) -> El {
    icon(glyph, size)
        .animated_changes()
        .icon_rotation(if open { open_radians } else { 0.0 })
}

/// Fixed-box in-place icon swap using additive dissolve (no midpoint alpha dip)
/// and crisp scale morphing (Send <-> Stop, Copy <-> Check, running <-> done).
pub fn icon_morph(
    slot_key: impl Into<String>,
    state_key: impl Into<String>,
    glyph: &'static Icon,
    icon_size: f32,
    box_size: f32,
    color: Color,
) -> El {
    let morph = Transition::scale(MORPH_SCALE)
        .allow_small_scale()
        .combined(Transition::opacity())
        .springs(QUICK_PAIR);
    row()
        .key(slot_key)
        .size(box_size, box_size)
        .center()
        .dissolve()
        .child_exit(morph)
        .a11y_hidden()
        .child(
            icon(glyph, icon_size)
                .key(state_key)
                .color(color)
                .transition(morph),
        )
}

/// Zero-bounce rolling numeric label built on `rune_kit::RollingText`.
///
/// Pass `trailing = true` for right-aligned counters (`+42`, `10s`) so adding
/// a digit widens to the left without shifting surviving columns.
pub fn rolling_count(
    key: impl Into<String>,
    label: impl Into<String>,
    value: f64,
    font: TextStyle,
    color: Color,
    trailing: bool,
    k: &Kit,
) -> El {
    let mut rolling = RollingText::new(key, label)
        .font(font)
        .color(color)
        .spring(SNAP)
        .value(value);
    if trailing {
        rolling = rolling.trailing();
    }
    rolling.build(k)
}

/// Right-aligned `+added -removed` rolling diff stat pair in tabular monospace.
pub fn rolling_diff_stat(key_prefix: &str, added: u32, removed: u32, t: &Theme) -> El {
    row()
        .key(format!("{key_prefix}-stat"))
        .gap(S6)
        .center_y()
        .child(rolling_count(
            format!("{key_prefix}-add"),
            format!("+{added}"),
            f64::from(added),
            t.mono_small,
            t.success,
            true,
            &t.k,
        ))
        .child(rolling_count(
            format!("{key_prefix}-del"),
            format!("-{removed}"),
            f64::from(removed),
            t.mono_small,
            t.danger,
            true,
            &t.k,
        ))
}

/// Optional horizontal linear shimmer overlay for active thinking and tool
/// rows.
///
/// Returns `None` when `active` is `false` so no loop track mounts and idle
/// windows request zero frames. Under Reduce Motion, Rune freezes the sweep at
/// its calm midpoint and stops scheduling frames.
pub fn shimmer_band(key: impl Into<String>, active: bool, tint: Color, radius: f32) -> Option<El> {
    if !active {
        return None;
    }
    let stops = [
        (0.0, Color::TRANSPARENT),
        (0.32, tint.alpha(0.05)),
        (0.50, tint.alpha(0.16)),
        (0.68, tint.alpha(0.05)),
        (1.0, Color::TRANSPARENT),
    ];
    let beam = el()
        .key("beam")
        .w_frac(SHIMMER_BEAM_FRAC)
        .h_full()
        .linear_gradient(FRAC_PI_2, stops)
        .looping(
            LoopSpec::slide(SHIMMER_PERIOD, [-1.0, 0.0], [1.0 / SHIMMER_BEAM_FRAC, 0.0])
                .easing(Easing::LINEAR),
        );
    Some(
        el().key(key)
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .clip_rounded(radius)
            .a11y_hidden()
            .child(beam),
    )
}

/// Shared-element glide for compact indicators (provider rail pill, segmented
/// tab thumb).
pub fn shared_pill(id: impl Into<String>, target: El) -> El {
    target.matched(id).motion(QUICK)
}

/// Shared-element surface flight that crops instead of stretching text as
/// aspect changes.
pub fn shared_surface(id: impl Into<String>, target: El) -> El {
    target.matched(id).matched_crop().motion(GLIDE)
}

/// Launches an element from `source` to `dest` across container clips (e.g.
/// composer -> timeline).
pub fn launch_from(tx: &mut Tx, source: impl Into<String>, dest: impl Into<String>) {
    tx.unclipped_flights();
    tx.launch(source, dest);
}

/// Width-scaled `SplitView` motion for sidebars, diff panels, and inspectors.
pub fn pane_motion() -> PaneMotion {
    PaneMotion::new()
        .spring(GLIDE)
        .scale_with_width(PANE_REF_WIDTH, PANE_MIN_DURATION, PANE_MAX_DURATION)
        .fade_tail(PANE_FADE_TAIL)
        .reduced_motion_fade(PANE_REDUCED_FADE)
}

/// Edge-sliding transition for docked drawers and bottom/side panels.
pub fn slide_panel(edge: Edge) -> Transition {
    Transition::move_edge(edge)
        .combined(Transition::opacity())
        .springs(GLIDE_PAIR)
}

/// Locks text layout to its destination width while an adjacent pane opens or
/// closes.
pub fn stable_content(target: El) -> El {
    target.hold_landing_width()
}

/// Pointer drag state for vertical list reordering with spring settle.
///
/// Tracks a `4pt` slop threshold before lifting; while held, the dragged row
/// tracks the pointer `1:1` (`Anim::INSTANT`) and displaced siblings step aside
/// on [`REFLOW`]. Releasing commits `(from, to)` or cancels cleanly back to
/// rest.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DragOrder {
    gap: f32,
    lift: Option<DragLift>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragLift {
    Pressed {
        index: usize,
    },
    Dragging {
        from: usize,
        to: usize,
        dy: f32,
        pitch: f32,
    },
}

impl DragOrder {
    /// Creates an idle drag reorder tracker with zero row gap.
    pub const fn new() -> Self {
        Self {
            gap: 0.0,
            lift: None,
        }
    }

    /// Sets the vertical spacing between adjacent rows in points.
    pub const fn gap(mut self, points: f32) -> Self {
        self.gap = if points > 0.0 { points } else { 0.0 };
        self
    }

    /// Whether row `index` is actively lifted and following the pointer.
    pub const fn is_lifted(&self, index: usize) -> bool {
        matches!(self.lift, Some(DragLift::Dragging { from, .. }) if from == index)
    }

    /// Vertical displacement in points for row `index` during an active drag.
    pub fn shift(&self, index: usize) -> f32 {
        let Some(DragLift::Dragging {
            from,
            to,
            dy,
            pitch,
        }) = self.lift
        else {
            return 0.0;
        };
        if index == from {
            dy
        } else if from < index && index <= to {
            -pitch
        } else if to <= index && index < from {
            pitch
        } else {
            0.0
        }
    }

    /// Updates drag state from a row's pointer gesture, returning `Some((from,
    /// to))` when a drop moves an item to a different index.
    pub fn handle(&mut self, index: usize, count: usize, drag: &Drag) -> Option<(usize, usize)> {
        match drag.phase {
            Phase::Down => {
                self.lift = Some(DragLift::Pressed { index });
                None
            }
            Phase::Move => {
                let (from, pitch) = match self.lift {
                    Some(DragLift::Dragging { from, pitch, .. }) => (from, pitch),
                    _ if drag.dy.abs() < DRAG_SLOP => return None,
                    Some(DragLift::Pressed { index: pressed }) => {
                        (pressed, (drag.h + self.gap).max(1.0))
                    }
                    None => (index, (drag.h + self.gap).max(1.0)),
                };
                let max_index = count.saturating_sub(1);
                let delta = (drag.dy / pitch).round();
                let to = shifted_index(from, delta, max_index);
                self.lift = Some(DragLift::Dragging {
                    from,
                    to,
                    dy: drag.dy,
                    pitch,
                });
                None
            }
            Phase::Up => {
                let lift = self.lift.take();
                match lift {
                    Some(DragLift::Dragging { from, to, .. })
                        if !drag.cancelled() && from != to =>
                    {
                        Some((from, to))
                    }
                    _ => None,
                }
            }
        }
    }
}

/// Applies flat lifted styling and pointer/spring offsets to a reorderable row.
///
/// The lifted row rises above siblings with an opaque fill and `1px` hairline
/// border (no drop shadow) while tracking `1:1`; siblings step aside and the
/// dropped row settles on [`REFLOW`].
pub fn drag_row<S: 'static>(
    target: El,
    order: DragOrder,
    index: usize,
    count: usize,
    lifted_fill: Color,
    lifted_border: Color,
    on_drag: impl Fn(&mut S, &mut Drag, &mut Tx) + 'static,
) -> El {
    let mut row_el = target
        .position_in_set(index + 1, count)
        .motion(REFLOW)
        .offset(0.0, order.shift(index))
        .on_drag(on_drag);
    if order.is_lifted(index) {
        row_el = row_el
            .raise()
            .offset_motion(Anim::INSTANT)
            .fill(lifted_fill)
            .border(HAIRLINE, lifted_border);
    }
    row_el
}

fn shifted_index(from: usize, delta: f32, max_index: usize) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "drag displacement is clamped to a small list index range"
    )]
    let steps = delta.clamp(-10_000.0, 10_000.0) as isize;
    if steps >= 0 {
        from.saturating_add(steps.unsigned_abs()).min(max_index)
    } else {
        from.saturating_sub(steps.unsigned_abs())
    }
}
