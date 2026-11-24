//! Live Working header and Completed Turn Fold (`"Worked for 5s >"`), driven
//! by zero-bounce critically-damped springs from [`crate::motion`].

use std::f32::consts::FRAC_PI_2;

use rune::{El, Tx, col, el, row, text};

use super::work_log;
use crate::{
    app::Desktop,
    model::{
        Item, ToolStatus, TurnView, format_clock_hm, format_turn_duration, work_group_summary,
    },
    motion,
    theme::{HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_SM, S2, S4, S6, S8, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// Renders the live Working header and real-time active work rows for an
/// in-progress turn.
pub fn live_working(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> El {
    let now = s.model.disclosure.tool_clock;
    let compacting = turn.compacting();
    let elapsed_ms = turn.elapsed_ms(now);
    let prefix = if compacting {
        "Compacting for "
    } else if elapsed_ms.is_some() {
        "Working for "
    } else {
        "Working"
    };

    let label_el = elapsed_ms.map_or_else(
        || text(prefix).font(t.ui).color(t.muted),
        |ms| {
            let secs = (ms / 1_000).max(0);
            let label = format_turn_duration(ms);
            #[expect(
                clippy::cast_precision_loss,
                reason = "elapsed turn seconds stay well within exact f64 range"
            )]
            let value = secs as f64;
            row()
                .center_y()
                .child(text(prefix).font(t.ui).color(t.muted))
                .child(motion::rolling_count(
                    format!("{}-elapsed", turn.key),
                    label,
                    value,
                    t.ui,
                    t.muted,
                    false,
                    &t.k,
                ))
        },
    );

    let header_row = row()
        .key(format!("{}-working-head", turn.key))
        .role("status")
        .label(if compacting { "Compacting" } else { "Working" })
        .w_full()
        .px(S4)
        .pt(S4)
        .pb(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .child(ui::radar(ICON_SMALL, t.accent, t))
        .child(label_el);

    let mut section =
        motion::fold_container(col().key(format!("{}-working", turn.key)).w_full().pb(S6))
            .child(header_row)
            .child(ui::rule(t).key("working-rule"));

    if !turn.work.is_empty() {
        let mut active_indices = Vec::new();
        let mut settled_indices = Vec::new();
        for (i, (item, streaming)) in turn.work.iter().enumerate() {
            let is_live = *streaming
                || matches!(
                    item,
                    Item::Tool(tool) if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
                );
            if is_live {
                active_indices.push(i);
            } else {
                settled_indices.push(i);
            }
        }
        let mut body = col()
            .key(format!("{}-live-work", turn.key))
            .w_full()
            .pt(S6)
            .gap(S2);

        if settled_indices.len() > 3 {
            let fold_key = format!("{}-live-earlier", turn.key);
            let open = s.model.timeline.is_open(
                s.model.selection.selected,
                &s.model.disclosure.expanded,
                &fold_key,
                None,
                false,
            );
            let settled_slice: Vec<(&Item, bool)> =
                settled_indices.iter().map(|&i| turn.work[i]).collect();
            body = body.child(live_earlier_fold(s, &fold_key, &settled_slice, open, t));
        } else {
            for i in settled_indices {
                let (item, streaming) = turn.work[i];
                body = body.child(work_log::entry_row(s, item, streaming, t));
            }
        }

        for i in active_indices {
            let (item, streaming) = turn.work[i];
            body = body.child(work_log::entry_row(s, item, streaming, t));
        }

        section = section.child(body);
    }

    section
}

fn live_earlier_fold(
    s: &Desktop,
    fold_key: &str,
    settled: &[(&Item, bool)],
    open: bool,
    t: &Theme,
) -> El {
    let summary = work_group_summary(settled);
    let header_key = format!("{fold_key}-head");
    let toggle_key = fold_key.to_owned();
    let anchor = header_key.clone();
    let trigger = row()
        .key(header_key)
        .role("button")
        .label(summary.clone())
        .expanded(open)
        .w_full()
        .min_h(crate::theme::S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::ToggleFold {
                    key: toggle_key.clone(),
                    legacy_key: None,
                    default_open: false,
                    anchor: Some(anchor.clone()),
                }),
                tx,
            );
        })
        .child(
            text(summary)
                .font(t.small)
                .color(t.muted)
                .group_color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0),
        )
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .key("chevron")
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        );

    let mut box_el = motion::fold_container(col().key(fold_key.to_owned()).w_full()).child(trigger);
    if open {
        box_el = box_el.child(motion::fold_row(
            col()
                .key(format!("{fold_key}-body"))
                .w_full()
                .gap(S2)
                .children(
                    settled
                        .iter()
                        .map(|(item, streaming)| work_log::entry_row(s, item, *streaming, t)),
                ),
        ));
    }
    box_el
}

/// Renders the hairline-underlined Completed Turn Fold (`"Worked for 5s >"`)
/// that collapses all intermediate reasoning and tool work once settled.
pub fn completed_fold(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> Option<El> {
    if turn.work.is_empty() {
        return None;
    }
    let m = &s.model;
    let fold_key = turn.fold_key();
    let legacy_key = turn.legacy_fold_key.as_deref();
    let default_open = turn.interrupted;
    let open = m.timeline.is_open(
        m.selection.selected,
        &m.disclosure.expanded,
        &fold_key,
        legacy_key,
        default_open,
    );

    let label = fold_label(turn);
    let failures = turn
        .work
        .iter()
        .filter(|(item, _)| matches!(item, Item::Tool(tool) if tool.status == ToolStatus::Failed))
        .count();
    let clock = turn.ended_at.and_then(format_clock_hm);

    let header_key = format!("{fold_key}-header");
    let toggle_key = fold_key.clone();
    let toggle_legacy = turn.legacy_fold_key.clone();
    let anchor = header_key.clone();

    let mut header = row()
        .key(header_key)
        .role("button")
        .label(label.clone())
        .expanded(open)
        .w_full()
        .px(S4)
        .pt(S4)
        .pb(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::ToggleFold {
                    key: toggle_key.clone(),
                    legacy_key: toggle_legacy.clone(),
                    default_open,
                    anchor: Some(anchor.clone()),
                }),
                tx,
            );
        })
        .child(
            text(label)
                .font(t.ui)
                .color(t.muted)
                .group_color(t.text)
                .truncate()
                .shrink(),
        )
        .child(
            motion::chevron(&ui::CHEVRON, ICON_MD, open, FRAC_PI_2)
                .key("chevron")
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(el().grow(1.0).min_w(0.0).a11y_hidden());

    if failures > 0 {
        header = header.child(
            text(format!("{failures} failed"))
                .font(t.micro)
                .color(t.danger),
        );
    }
    if let Some(clock) = clock {
        header = header.child(
            text(clock)
                .font(t.micro)
                .color(t.faint)
                .group_opacity(0.0, 1.0),
        );
    }

    let mut section = motion::fold_container(col().key(fold_key).w_full().pb(S6))
        .child(header)
        .child(ui::rule(t).key("fold-rule"));

    if open {
        section = section.child(motion::fold_row(
            col()
                .key(format!("{}-fold-body", turn.key))
                .w_full()
                .pt(S6)
                .pb(S4)
                .child(work_log::work_group(s, turn, t)),
        ));
    }

    Some(section)
}

fn fold_label(turn: &TurnView<'_>) -> String {
    let only_thought = turn
        .work
        .iter()
        .all(|(item, _)| matches!(item, Item::Thinking { .. }));
    let duration = turn
        .duration_ms()
        .filter(|&ms| ms >= 500)
        .map(format_turn_duration);
    if turn.interrupted {
        return duration.map_or_else(
            || "You stopped".to_owned(),
            |dur| format!("You stopped after {dur}"),
        );
    }
    if turn.continued {
        return duration.map_or_else(
            || "Continued".to_owned(),
            |dur| format!("Continued for {dur}"),
        );
    }
    if only_thought {
        return duration.map_or_else(|| "Thought".to_owned(), |dur| format!("Thought for {dur}"));
    }
    duration.map_or_else(
        || {
            let count = turn.work.len();
            format!(
                "Worked · {count} {}",
                crate::model::plural(count, "step", "steps")
            )
        },
        |dur| format!("Worked for {dur}"),
    )
}
