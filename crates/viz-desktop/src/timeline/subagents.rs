//! Compact work-log rows for subagent interactions.

use rune::{El, Tx, el, icon, row, text};

use super::work_log;
use crate::{
    app::Desktop,
    model::{SubagentCall, Tool, delegated_task, local_child, subagent_title},
    theme::{HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_SM, S2, S4, S6, S24, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// Compact `24pt` work-log row for non-spawn subagent interactions (message,
/// status read, list, cancel).
pub fn subagent_interaction_row(s: &Desktop, tool: &Tool, call: SubagentCall, t: &Theme) -> El {
    let m = &s.model;
    let verb = match call {
        SubagentCall::Spawn => "Started",
        SubagentCall::Message => "Messaged",
        SubagentCall::Read => "Read",
        SubagentCall::List => "Listed tasks",
        SubagentCall::Cancel => "Stopped",
    };
    let task = delegated_task(m, tool);
    let thread = local_child(m, tool, task);
    let glyph = thread.map_or(&ui::BOT, |thread| ui::harness_icon(&thread.harness));
    let name = (tool.child.is_some() || task.is_some())
        .then(|| subagent_title(tool, task, thread))
        .filter(|title| !title.is_empty());
    let verb = match (call, name) {
        (SubagentCall::Read, None) => "Checked thread activity",
        (SubagentCall::Message, None) => "Sent a message",
        (SubagentCall::Cancel, None) => "Stopped a task",
        _ => verb,
    };
    let label = name.map_or_else(|| verb.to_owned(), |name| format!("{verb} {name}"));
    let mut line = row()
        .key(tool.key.clone())
        .label(label)
        .w_full()
        .min_h(S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .child(work_log::icon_slot(
            icon(glyph, ICON_MD).color(t.faint).group_color(t.text),
        ))
        .child(
            text(verb)
                .font(t.ui)
                .color(t.text_secondary)
                .group_color(t.text),
        )
        .children(name.map(|name| {
            text(name)
                .font(t.ui)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
        }));

    let environment = tool.child_environment.clone();
    if let Some(child) = tool
        .child
        .filter(|_| task.is_none_or(|task| task.reachable))
    {
        line = line
            .role("link")
            .help("Open thread")
            .hover(t.hover_wash)
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: child,
                        environment: environment.clone(),
                    }),
                    tx,
                );
            })
            .child(
                icon(&ui::CHEVRON, ICON_SMALL)
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            );
    }

    line.child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(work_log::tool_status_meta(tool, m.disclosure.tool_clock, t))
}
