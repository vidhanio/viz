//! Interactive motion primitives: reflow, fold, morph, rolling counters,
//! shimmer, shared elements, and drag reorder.

use std::f32::consts::FRAC_PI_2;

use rune::{
    Anim, Color, Drag, Easing, El, GrowthSpec, Icon, LoopSpec, Phase, TextStyle, Transition, Tx,
    el, icon, row,
};
use rune_kit::{Kit, RollingText};

use super::{
    presence::fade,
    tokens::{
        DRAG_SLOP, FOLD_FEATHER, GLIDE, MORPH_SCALE, PRESS_SCALE, QUICK, QUICK_PAIR, REFLOW,
        REFLOW_PAIR, SETTLE, SHIMMER_BEAM_FRAC, SHIMMER_PERIOD, SNAP,
    },
};
use crate::theme::{HAIRLINE, S6, Theme};

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
