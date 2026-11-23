//! Persistent tucked Tasks Dock above the composer. Only mounts when the
//! open thread has delegated tasks.

use std::{
    borrow::Cow,
    f32::consts::{FRAC_PI_2, PI},
};

use rune::{El, GrowthSpec, ScrollSpec, col, el, icon, row, text};
use viz_client::protocol::{Task, TaskStatus};

use super::task_row::{child_summary, model_detail, row_view};
use crate::{
    app::Desktop,
    model::{Model, thread_title},
    motion,
    theme::{HAIRLINE, ICON_SMALL, S4, S6, S8, S12, S24, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// `DisclosureState::expanded` key for whether the Tasks Dock is expanded.
pub const DOCK_KEY: &str = "tasks-dock";
/// `DisclosureState::expanded` key for revealing settled tasks past the first
/// two.
const EARLIER_KEY: &str = "tasks-earlier";
/// Collapsed bar height (excluding the tucked bottom overlap).
const BAR_HEIGHT: f32 = 34.0;
/// Maximum height of the scrollable task list when expanded.
const LIST_MAX_HEIGHT: f32 = 190.0;
/// Progress hairline width in the collapsed dock.
const PROGRESS_WIDTH: f32 = 44.0;
/// Number of settled tasks shown directly before folding the rest.
const RECENT_SETTLED: usize = 2;

/// Renders the persistent Tasks Dock tucked above the composer when the
/// selected thread has at least one delegated task.
pub fn dock(s: &Desktop, t: &Theme) -> Option<El> {
    let m = &s.model;
    let parent = m.selection.selected?;
    let children: Vec<&Task> = m
        .tasks()
        .iter()
        .filter(|task| task.parent == parent)
        .collect();
    if children.is_empty() {
        return None;
    }
    let expanded = m.disclosure.expanded.contains(DOCK_KEY);
    let (active, settled): (Vec<&Task>, Vec<&Task>) = children
        .iter()
        .copied()
        .partition(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Queued));
    let any_failed = children
        .iter()
        .any(|task| task.status == TaskStatus::Failed);
    let body = if expanded {
        expanded_body(m, &active, &settled, any_failed, t)
    } else {
        collapsed_bar(
            m,
            &children,
            &active,
            settled.len(),
            children.len(),
            any_failed,
            t,
        )
    };
    Some(
        ui::tucked_deck(format!("tasks-dock-{parent}"), 10, t.surface, t)
            .role("group")
            .label("Tasks")
            .transition(motion::tucked_deck())
            .child(body),
    )
}

fn lead_mark(active: bool, any_failed: bool, t: &Theme) -> El {
    if active {
        ui::radar(ICON_SMALL, t.accent, t)
    } else if any_failed {
        ui::dot(t.danger)
    } else {
        ui::dot(t.success)
    }
}

fn collapsed_bar(
    m: &Model,
    all: &[&Task],
    active: &[&Task],
    done: usize,
    total: usize,
    any_failed: bool,
    t: &Theme,
) -> El {
    row()
        .key("tasks-dock-collapsed")
        .role("button")
        .label("Expand tasks")
        .expanded(false)
        .w_full()
        .h(BAR_HEIGHT)
        .px(S12)
        .gap(S8)
        .center_y()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| {
            s.dispatch_local(Msg::Timeline(TimelineMsg::ToggleExpanded(
                DOCK_KEY.to_owned(),
            )));
        })
        .child(lead_mark(!active.is_empty(), any_failed, t))
        .child(medallion_stack(m, all, t))
        .child(collapsed_summary(m, active, done, total, any_failed, t))
        .child(progress_hairline(done, total, t))
        .child(
            icon(&ui::CHEVRON_UP, ICON_SMALL + 1.0)
                .icon_rotation(0.0)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

fn medallion_stack(m: &Model, tasks: &[&Task], t: &Theme) -> El {
    let mut stack = row().gap(S4).center_y().a11y_hidden();
    for task in tasks.iter().take(3) {
        let child = child_summary(m, task);
        let glyph = child.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
        stack = stack.child(icon(glyph, ICON_SMALL).color(t.muted));
    }
    stack
}

fn collapsed_summary(
    m: &Model,
    active: &[&Task],
    done: usize,
    total: usize,
    any_failed: bool,
    t: &Theme,
) -> El {
    if active.len() == 1 {
        let task = active[0];
        let child = child_summary(m, task);
        let title = child.map_or(task.title.as_str(), thread_title);
        let detail = child
            .and_then(model_detail)
            .or_else(|| (done > 0).then(|| Cow::Owned(format!("1 running, {done} done"))));
        let mut line = row()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .gap(S6)
            .center_y()
            .child(
                text(title)
                    .font(t.ui_medium)
                    .color(t.text)
                    .shrink()
                    .truncate(),
            );
        if let Some(detail) = detail {
            line = line
                .child(text("·").font(t.small).color(t.faint).a11y_hidden())
                .child(
                    text(detail)
                        .font(t.small)
                        .color(t.faint)
                        .shrink()
                        .truncate(),
                );
        }
        return line;
    }
    let summary = counts_summary(active.len(), done, total, any_failed);
    row().grow(1.0).shrink().min_w(0.0).center_y().child(
        text(summary)
            .font(if active.is_empty() { t.ui } else { t.ui_medium })
            .color(if active.is_empty() { t.muted } else { t.text })
            .group_color(t.text)
            .truncate(),
    )
}

fn counts_summary(running: usize, settled: usize, total: usize, any_failed: bool) -> String {
    match (running, settled) {
        (r, 0) if r > 0 => format!("{r} running"),
        (r, d) if r > 0 => format!("{r} running · {d} done"),
        (0, _) if any_failed => format!("{total} tasks · failed"),
        (0, d) => format!("{d} done"),
        _ => format!("{total} tasks"),
    }
}

fn progress_hairline(done: usize, total: usize, t: &Theme) -> El {
    let ratio = u16::try_from(done).unwrap_or(u16::MAX);
    let denom = u16::try_from(total.max(1)).unwrap_or(1);
    let fill_w = (f32::from(ratio) / f32::from(denom)) * PROGRESS_WIDTH;
    el().w(PROGRESS_WIDTH)
        .h(3.0)
        .radius(1.5)
        .fill(t.hairline)
        .a11y_hidden()
        .child(
            el().w(fill_w.max(if done > 0 { 4.0 } else { 0.0 }))
                .h(3.0)
                .radius(1.5)
                .fill(t.accent),
        )
}

fn expanded_body(
    m: &Model,
    active: &[&Task],
    settled: &[&Task],
    any_failed: bool,
    t: &Theme,
) -> El {
    let summary = counts_summary(
        active.len(),
        settled.len(),
        active.len() + settled.len(),
        any_failed,
    );
    let header = row()
        .key("tasks-dock-header")
        .role("button")
        .label("Collapse tasks")
        .expanded(true)
        .w_full()
        .h(BAR_HEIGHT)
        .px(S12)
        .gap(S8)
        .center_y()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| {
            s.dispatch_local(Msg::Timeline(TimelineMsg::ToggleExpanded(
                DOCK_KEY.to_owned(),
            )));
        })
        .child(lead_mark(!active.is_empty(), any_failed, t))
        .child(text("Tasks").font(t.ui_medium).color(t.text))
        .child(text("·").font(t.small).color(t.faint).a11y_hidden())
        .child(
            text(summary)
                .font(t.small)
                .color(t.muted)
                .grow(1.0)
                .truncate(),
        )
        .child(
            icon(&ui::CHEVRON_UP, ICON_SMALL + 1.0)
                .icon_rotation(PI)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        );
    let (recent_settled, earlier) = if settled.len() > RECENT_SETTLED {
        settled.split_at(RECENT_SETTLED)
    } else {
        (settled, &[][..])
    };
    let mut inner = col()
        .key("tasks-dock-rows")
        .w_full()
        .growth(GrowthSpec::default());
    for task in active.iter().chain(recent_settled) {
        inner = inner.child(row_view(m, task, t));
    }
    if !earlier.is_empty() {
        let shown = m.disclosure.expanded.contains(EARLIER_KEY);
        inner = inner.child(earlier_toggle(earlier.len(), shown, t));
        if shown {
            for task in earlier {
                inner = inner.child(row_view(m, task, t));
            }
        }
    }
    let list = col()
        .key("tasks-dock-list")
        .w_full()
        .max_h(LIST_MAX_HEIGHT)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S12)
        .child(inner);
    col()
        .w_full()
        .growth(GrowthSpec::default())
        .child(header)
        .child(ui::rule(t))
        .child(list)
}

fn earlier_toggle(count: usize, shown: bool, t: &Theme) -> El {
    let label = if count == 1 {
        "1 earlier task".to_owned()
    } else {
        format!("{count} earlier tasks")
    };
    row()
        .key(EARLIER_KEY)
        .role("button")
        .label(&label)
        .expanded(shown)
        .w_full()
        .h(S24 + S4)
        .px(S12)
        .gap(S6)
        .center_y()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| {
            s.dispatch_local(Msg::Timeline(TimelineMsg::ToggleExpanded(
                EARLIER_KEY.to_owned(),
            )));
        })
        .child(
            icon(&ui::CHEVRON, ICON_SMALL)
                .icon_rotation(if shown { FRAC_PI_2 } else { 0.0 })
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.small)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
}
