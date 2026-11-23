//! Timeline message rows: user bubbles, assistant prose, reasoning traces,
//! plan checklists, turn-end copy actions, and error notices.

use rune::{El, Icon, Tx, col, el, icon, row, text};
use rune_kit::Markdown;

use super::activity::{line, mark};
use crate::{
    app::Desktop,
    attachments,
    model::{PlanStep, ToolStatus, TurnEnd, User},
    motion,
    theme::{HAIRLINE, ICON, ICON_SMALL, RADIUS_INNER, S2, S4, S8, S12, Theme},
    ui::{self, Card},
    update::{Msg, TimelineMsg},
};

/// The widest a user bubble gets, as a share of the reading column.
const BUBBLE_FRAC: f32 = 0.8;

/// A user message: a bubble on a neutral surface at the right of the column,
/// with a quiet hover-revealed Copy action.
pub(super) fn mine(user: &User, t: &Theme) -> El {
    let mut message = Card::Message
        .build(t)
        .key(&user.key)
        .role("article")
        .label("You");
    if !user.text.is_empty() {
        message = message.child(
            text(&user.text)
                .key("text")
                .font(t.body)
                .color(t.text)
                .wrap()
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    }
    if !user.attachments.is_empty() {
        message = message.child(
            row()
                .key("message-attachments")
                .self_end()
                .gap(S4)
                .flex_wrap()
                .justify_end()
                .children(user.attachments.iter().enumerate().map(|(index, name)| {
                    attachments::chip(name, t).key(format!("attachment-{index}"))
                })),
        );
    }
    let footer = user_actions(user, t);
    row()
        .key(format!("{}-row", user.key))
        .w_full()
        .justify_end()
        .hover_group()
        .child(el().w_frac(1.0 - BUBBLE_FRAC))
        .child(
            col()
                .min_w(0.0)
                .shrink()
                .items_end()
                .gap(S2)
                .child(message)
                .children(footer),
        )
}

fn user_actions(user: &User, t: &Theme) -> Option<El> {
    if user.text.is_empty() {
        return None;
    }
    let copied = user.text.clone();
    Some(
        row()
            .key(format!("{}-actions", user.key))
            .h(22.0)
            .gap(S8)
            .center_y()
            .px(S4)
            .group_opacity(0.0, 1.0)
            .child(action_chip(
                "copy-user",
                &ui::COPY,
                "Copy",
                t.faint,
                t.text,
                move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::CopyText(copied.clone())), tx);
                },
                t,
            )),
    )
}

fn action_chip(
    key: &str,
    glyph: &'static Icon,
    label: &'static str,
    rest: rune::Color,
    hover: rune::Color,
    on_press: impl Fn(&mut Desktop, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label)
        .h(22.0)
        .gap(S4)
        .center_y()
        .radius(RADIUS_INNER)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(motion::PRESS_SCALE)
        .on_press_tx(on_press)
        .child(
            icon(glyph, ICON_SMALL)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.small)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
}

/// Quiet hover-revealed Copy action beneath an assistant turn.
pub(super) fn turn_end(end: &TurnEnd, t: &Theme) -> Option<El> {
    if end.answer.is_empty() {
        return None;
    }
    let copied = end.answer.clone();
    Some(
        col()
            .key(end.key.clone())
            .w_full()
            .pt(S8)
            .hover_group_shared(end.key.as_str())
            .child(
                row()
                    .key(format!("{}-actions", end.key))
                    .h(22.0)
                    .gap(S12)
                    .center_y()
                    .group_opacity(0.0, 1.0)
                    .child(action_chip(
                        "copy-answer",
                        &ui::COPY,
                        "Copy",
                        t.faint,
                        t.text,
                        move |s: &mut Desktop, tx| {
                            s.dispatch(Msg::Timeline(TimelineMsg::CopyText(copied.clone())), tx);
                        },
                        t,
                    )),
            ),
    )
}

/// Assistant prose rendered as Markdown.
pub(super) fn prose(key: &str, source: &str, streaming: bool, t: &Theme) -> El {
    Markdown::new(source.to_owned())
        .style(t.markdown())
        .streaming(streaming)
        .selectable(true)
        .on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
        })
        .build(&t.k)
        .key(key.to_owned())
        .w_full()
}

/// Reasoning trace beside a thin rail in the icon column.
pub(super) fn thought(key: &str, thought: &str, streaming: bool, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .py(S4)
        .gap(S8)
        .items_stretch()
        .role("group")
        .label(if streaming { "Thinking" } else { "Thought" })
        .child(
            col()
                .w(ICON)
                .items_center()
                .a11y_hidden()
                .child(el().w(S2).grow(1.0).radius(HAIRLINE).fill(t.hairline)),
        )
        .child(
            Markdown::new(thought.to_owned())
                .style(t.markdown().body_font(t.ui).text(t.muted).link(t.text))
                .streaming(streaming)
                .selectable(true)
                .on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
                })
                .build(&t.k)
                .grow(1.0)
                .min_w(0.0),
        )
}

/// The run's checklist, in the activity grid: a heading row with progress,
/// then each step's mark in the icon column.
pub(super) fn plan(key: &str, steps: &[PlanStep], t: &Theme) -> El {
    let line_height = t.ui.line_height;
    let done = steps
        .iter()
        .filter(|step| step.status == ToolStatus::Done)
        .count();
    col()
        .key(key)
        .w_full()
        .role("list")
        .label("Plan")
        .gap(S4)
        .child(
            line("heading", mark(&ui::PLAN, t))
                .child(text("Plan").font(t.ui_medium).color(t.text))
                .child(
                    text(format!("{done} of {}", steps.len()))
                        .font(t.small)
                        .color(t.faint),
                ),
        )
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, label, ink) = match step.status {
                ToolStatus::Done => (
                    icon(&ui::CHECK, ICON_SMALL).color(t.success),
                    "Done",
                    t.muted,
                ),
                ToolStatus::Running => (ui::spinner(t), "In progress", t.text),
                _ => (
                    icon(&ui::CIRCLE, ICON_SMALL).color(t.faint),
                    "Pending",
                    t.muted,
                ),
            };
            row()
                .key(format!("step-{index}"))
                .role("listitem")
                .description(label)
                .w_full()
                .gap(S8)
                .items_start()
                .child(ui::lead(mark, line_height))
                .child(
                    text(&step.text)
                        .font(t.ui)
                        .color(ink)
                        .grow(1.0)
                        .shrink()
                        .wrap(),
                )
        }))
}

/// An answer that didn't come, in danger ink.
pub(super) fn notice(key: &str, message: &str, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .role("alert")
        .w_full()
        .gap(S8)
        .items_start()
        .child(ui::lead(
            icon(&ui::CIRCLE_X, ICON_SMALL).color(t.danger),
            t.ui.line_height,
        ))
        .child(
            text(message.to_owned())
                .font(t.ui)
                .color(t.danger)
                .wrap()
                .selectable(true)
                .grow(1.0)
                .shrink(),
        )
}
