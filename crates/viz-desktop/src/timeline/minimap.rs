//! Left-gutter `TimelineMinimap`: static-width turn ticks (zero CSS fisheye),
//! active/in-view highlight, prev/next step controls, click jump via
//! `Tx::scroll_key_center_at`, and flat `320pt` hover preview popover.

use rune::{El, Tx, col, el, row, text};
use rune_kit::{IconButtonVariant, Placement};

use crate::{
    app::Desktop,
    model::{TurnView, format_clock_hm},
    theme::{
        CARD_INSET, CHAT_CONTENT_MAX_WIDTH, HAIRLINE, RADIUS_PILL, S2, S6, S12, S24, Size, Theme,
    },
    ui::{self, BadgeSize, Tone},
    update::{Msg, TimelineMsg},
};

/// Minimum left gutter width required to mount the minimap without crowding the
/// centered reading lane (`44pt`).
const MIN_GUTTER_WIDTH: f32 = 44.0;
/// Active / hovered tick bar width (`22pt`).
const TICK_ACTIVE_W: f32 = 22.0;
/// Resting tick bar width (`12pt`).
const TICK_REST_W: f32 = 12.0;
/// Tick bar stroke thickness (`2.5pt`).
const TICK_H: f32 = 2.5;
/// Flat hover preview popover width (`320pt`).
const PREVIEW_WIDTH: f32 = 320.0;
/// Target vertical midpoint in the timeline viewport when jumping to a turn.
const JUMP_CENTER_Y: f32 = 180.0;

/// Renders the left-gutter turn minimap (plus any active hover preview popover
/// pushed onto `layer`) when `turns.len() >= 2` and the left gutter is wide
/// enough.
pub fn minimap(s: &Desktop, turns: &[TurnView<'_>], t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    if turns.len() < 2 {
        return None;
    }
    let m = &s.model;
    let sidebar_w = if m.panels.sidebar.open {
        m.panels.sidebar.width
    } else {
        0.0
    };
    let diff_w = if m.panels.diff.open {
        m.panels.diff.width
    } else {
        0.0
    };
    let details_w = if m.selection.selected.is_some() && crate::model::git::inline(m) {
        crate::model::git::CARD_WIDTH + crate::model::git::CARD_INSET
    } else {
        0.0
    };
    let main_w = (m.panels.window_width - sidebar_w - diff_w - details_w).max(0.0);
    let gutter = (main_w - CHAT_CONTENT_MAX_WIDTH) * 0.5;
    if gutter < MIN_GUTTER_WIDTH {
        return None;
    }

    let current = m
        .timeline
        .visible_turns
        .iter()
        .copied()
        .next_back()
        .or_else(|| turns.len().checked_sub(1))
        .unwrap_or(0);
    let prev_target = current.checked_sub(1).and_then(|i| turns.get(i));
    let next_target = turns.get(current + 1);

    let prev_btn = step_button(
        "minimap-prev",
        &ui::CHEVRON_UP,
        "Previous turn",
        prev_target.map(|turn| turn.key.clone()),
        t,
    );
    let next_btn = step_button(
        "minimap-next",
        &ui::CHEVRON_OPEN,
        "Next turn",
        next_target.map(|turn| turn.key.clone()),
        t,
    );

    let mut ticks = col()
        .key("minimap-ticks")
        .role("navigation")
        .label("Conversation turns")
        .items_start()
        .gap(S2);

    for turn in turns {
        let tick_key = format!("minimap-tick-{}", turn.index);
        let hovered = m.timeline.hovered_minimap == Some(turn.index);
        let in_view = m.timeline.visible_turns.contains(&turn.index)
            || (m.timeline.visible_turns.is_empty() && turn.index + 1 == turns.len());
        ticks = ticks.child(minimap_tick(&tick_key, turn, hovered, in_view, t));
        if hovered {
            layer.push(preview_popover(&tick_key, turn, turns.len(), t));
        }
    }

    Some(
        col()
            .key("timeline-minimap")
            .w(28.0)
            .pin(Some(S12), Some(S24), None, Some(S24))
            .center()
            .child(
                col()
                    .items_start()
                    .gap(S6)
                    .child(prev_btn)
                    .child(ticks)
                    .child(next_btn),
            ),
    )
}

fn step_button(
    key: &str,
    glyph: &'static rune::Icon,
    label: &str,
    target: Option<String>,
    t: &Theme,
) -> El {
    ui::icon_button(glyph, label, IconButtonVariant::GhostMuted, Size::Micro)
        .disabled(target.is_none())
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            if let Some(target_key) = &target {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::JumpToTurn {
                        key: target_key.clone(),
                        center_y: JUMP_CENTER_Y,
                    }),
                    tx,
                );
            }
        })
        .build(&t.k)
        .key(key)
}

fn minimap_tick(
    tick_key: &str,
    turn: &TurnView<'_>,
    hovered: bool,
    in_view: bool,
    t: &Theme,
) -> El {
    let idx = turn.index;
    let width = if hovered || in_view {
        TICK_ACTIVE_W
    } else {
        TICK_REST_W
    };
    let color = if turn.live {
        t.accent
    } else if hovered || in_view {
        t.text
    } else {
        t.faint.alpha(0.55)
    };
    let target_key = turn.key.clone();
    row()
        .key(tick_key.to_owned())
        .role("button")
        .label(format!("Jump to turn {}", idx + 1))
        .w(S24)
        .h(S12)
        .items_center()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_hover_change(move |s: &mut Desktop, over: bool, tx: &mut Tx| {
            let next = if over {
                Some(idx)
            } else if s.model.timeline.hovered_minimap == Some(idx) {
                None
            } else {
                return;
            };
            s.dispatch(Msg::Timeline(TimelineMsg::HoverMinimap(next)), tx);
        })
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::JumpToTurn {
                    key: target_key.clone(),
                    center_y: JUMP_CENTER_Y,
                }),
                tx,
            );
        })
        .child(el().w(width).h(TICK_H).radius(RADIUS_PILL).fill(color))
}

fn preview_popover(anchor_key: &str, turn: &TurnView<'_>, total: usize, t: &Theme) -> El {
    let prompt = clean_preview(turn.prompt_preview());
    let reply = clean_preview(turn.reply_preview());
    let clock = turn.ended_at.or(turn.started_at).and_then(format_clock_hm);

    let mut header = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            text(format!("Turn {} of {total}", turn.index + 1))
                .font(t.micro_medium)
                .color(t.muted),
        )
        .child(el().grow(1.0).min_w(0.0).a11y_hidden());

    if turn.live {
        header = header.child(
            ui::badge("Working")
                .tone(Tone::Accent)
                .size(BadgeSize::Nano)
                .dot()
                .build(t)
                .key("live"),
        );
    } else if turn.interrupted {
        header = header.child(
            ui::badge("Stopped")
                .tone(Tone::Warning)
                .size(BadgeSize::Nano)
                .build(t)
                .key("stopped"),
        );
    }
    if let Some(clock) = clock {
        header = header.child(text(clock).font(t.micro).color(t.faint));
    }

    let mut body = col()
        .w_full()
        .padding(CARD_INSET)
        .gap(S6)
        .child(header)
        .child(
            text(prompt)
                .font(t.small_medium)
                .color(t.text)
                .wrap()
                .max_lines(2),
        );
    if !reply.is_empty() {
        body = body.child(text(reply).font(t.small).color(t.muted).wrap().max_lines(2));
    }

    ui::anchored_popover(
        format!("minimap-preview-{}", turn.index),
        anchor_key,
        Placement::RightStart,
        PREVIEW_WIDTH,
        body,
        t,
    )
}

fn clean_preview(text: &str) -> String {
    text.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_owned()
}
