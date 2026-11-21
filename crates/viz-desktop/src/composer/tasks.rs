//! Persistent tucked Tasks Dock above the composer, replacing the old
//! top-right tasks panel. Only mounts when the open thread has delegated tasks.

use std::{
    borrow::Cow,
    f32::consts::{FRAC_PI_2, PI},
};

use rune::{El, GrowthSpec, ScrollSpec, Spring, SpringPair, Transition, col, el, icon, row, text};
use viz_client::protocol::{Task, TaskStatus, ThreadSummary};

use crate::{
    app::{Desktop, thread_title},
    server::Command,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_INNER, S2, S4, S6, S8, S12, S24, Theme},
    ui,
};

/// `Desktop::expanded` key for whether the Tasks Dock is expanded.
pub const DOCK_KEY: &str = "tasks-dock";
/// `Desktop::expanded` key for revealing settled tasks past the first two.
const EARLIER_KEY: &str = "tasks-earlier";
/// Collapsed bar height (excluding the tucked bottom overlap).
const BAR_HEIGHT: f32 = 34.0;
/// Maximum height of the scrollable task list when expanded.
const LIST_MAX_HEIGHT: f32 = 190.0;
/// Minimum height of one task row in the expanded dock.
const ROW_MIN_HEIGHT: f32 = 44.0;
/// Progress hairline width in the collapsed dock.
const PROGRESS_WIDTH: f32 = 44.0;
/// Number of settled tasks shown directly before folding the rest.
const RECENT_SETTLED: usize = 2;

/// Renders the persistent Tasks Dock tucked above the composer, when the
/// selected thread has at least one delegated task.
pub fn dock(s: &Desktop, t: &Theme) -> Option<El> {
    let parent = s.selected?;
    let children: Vec<&Task> = s
        .tasks
        .iter()
        .filter(|task| task.parent == parent)
        .collect();
    if children.is_empty() {
        return None;
    }
    let expanded = s.expanded.contains(DOCK_KEY);
    let (active, settled): (Vec<&Task>, Vec<&Task>) = children
        .iter()
        .copied()
        .partition(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Queued));
    let any_failed = children
        .iter()
        .any(|task| task.status == TaskStatus::Failed);
    let active_ids: Vec<String> = active.iter().map(|task| task.id.clone()).collect();
    let total = children.len();
    let done = settled.len();

    let body = if expanded {
        expanded_body(s, &active, &settled, &active_ids, any_failed, t)
    } else {
        collapsed_bar(
            s,
            &children,
            &active,
            &active_ids,
            done,
            total,
            any_failed,
            t,
        )
    };

    let motion = Transition::offset(0.0, S12)
        .combined(Transition::opacity())
        .springs(SpringPair::new(
            Spring::new(0.22, 0.0),
            Spring::new(0.15, 0.0),
        ));
    Some(
        ui::tucked_deck(format!("tasks-dock-{parent}"), 10, t.surface, t)
            .role("group")
            .label("Tasks")
            .transition(motion)
            .child(body),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "collapsed dock summarizes active tasks, progress ratio, and stop targets"
)]
fn collapsed_bar(
    s: &Desktop,
    all: &[&Task],
    active: &[&Task],
    active_ids: &[String],
    done: usize,
    total: usize,
    any_failed: bool,
    t: &Theme,
) -> El {
    let lead_mark = if !active.is_empty() {
        ui::radar(ICON_SMALL, t.accent, t)
    } else if any_failed {
        ui::dot(t.danger)
    } else {
        ui::dot(t.success)
    };
    let medallions = medallion_stack(s, all, t);
    let summary = collapsed_summary(s, active, done, total, any_failed, t);
    let progress = progress_hairline(done, total, t);
    let mut bar = row()
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
        .on_press(|s: &mut Desktop| s.toggle(DOCK_KEY))
        .child(lead_mark)
        .child(medallions)
        .child(summary)
        .child(progress);
    if s.connected() && !active_ids.is_empty() {
        bar = bar.child(stop_all_button(active_ids.to_vec(), t));
    }
    bar.child(
        icon(&ui::CHEVRON_UP, ICON_SMALL + 1.0)
            .icon_rotation(0.0)
            .color(t.muted)
            .group_color(t.text)
            .a11y_hidden(),
    )
}

fn medallion_stack(s: &Desktop, tasks: &[&Task], t: &Theme) -> El {
    let mut stack = row().gap(S4).center_y().a11y_hidden();
    for task in tasks.iter().take(3) {
        let child = child_summary(s, task);
        let glyph = child.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
        stack = stack.child(icon(glyph, ICON_SMALL).color(t.muted));
    }
    stack
}

fn collapsed_summary(
    s: &Desktop,
    active: &[&Task],
    done: usize,
    total: usize,
    any_failed: bool,
    t: &Theme,
) -> El {
    if active.len() == 1 {
        let task = active[0];
        let child = child_summary(s, task);
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
                        .font(t.mono_small)
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
    s: &Desktop,
    active: &[&Task],
    settled: &[&Task],
    active_ids: &[String],
    any_failed: bool,
    t: &Theme,
) -> El {
    let lead_mark = if !active.is_empty() {
        ui::radar(ICON_SMALL, t.accent, t)
    } else if any_failed {
        ui::dot(t.danger)
    } else {
        ui::dot(t.success)
    };
    let summary = counts_summary(
        active.len(),
        settled.len(),
        active.len() + settled.len(),
        any_failed,
    );
    let mut header = row()
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
        .on_press(|s: &mut Desktop| s.toggle(DOCK_KEY))
        .child(lead_mark)
        .child(text("Tasks").font(t.ui_medium).color(t.text))
        .child(text("·").font(t.small).color(t.faint).a11y_hidden())
        .child(
            text(summary)
                .font(t.small)
                .color(t.muted)
                .grow(1.0)
                .truncate(),
        );
    if s.connected() && !active_ids.is_empty() {
        header = header.child(stop_all_button(active_ids.to_vec(), t));
    }
    header = header.child(
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
        inner = inner.child(task_row(s, task, t));
    }
    if !earlier.is_empty() {
        let shown = s.expanded.contains(EARLIER_KEY);
        inner = inner.child(earlier_toggle(earlier.len(), shown, t));
        if shown {
            for task in earlier {
                inner = inner.child(task_row(s, task, t));
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
        .on_press(|s: &mut Desktop| s.toggle(EARLIER_KEY))
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

fn task_row(s: &Desktop, task: &Task, t: &Theme) -> El {
    let running = matches!(task.status, TaskStatus::Running | TaskStatus::Queued);
    let (mark, state_label) = task_status_mark(task, t);
    let child = child_summary(s, task);
    let harness = child.map(|thread| (thread.harness.as_str(), thread.harness_name.as_str()));
    let title = child.map_or(task.title.as_str(), thread_title);
    let remote = s.environment.as_deref() != Some(task.environment.as_str());
    let text_col = task_row_text(title, remote, &task.environment, child, task, running, t);
    let trailing = task_row_trailing(s, task, running, state_label, t);
    let mut row_el = row()
        .key(format!("dock-task-{}", task.id))
        .description(state_label)
        .w_full()
        .min_h(ROW_MIN_HEIGHT)
        .px(S12)
        .py(S6)
        .gap(S8)
        .center_y()
        .child(el().w(14.0).center().child(mark).a11y_hidden())
        .child(ui::harness_mark(
            harness,
            task.status == TaskStatus::Running,
            t,
        ))
        .child(text_col)
        .child(trailing);
    if task.reachable
        && let Some(thread) = task.thread
    {
        let environment = task.environment.clone();
        row_el = row_el
            .role("link")
            .label(title)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .hover_group()
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.open_in_environment(environment.clone(), Some(thread), tx);
            });
    }
    row_el
}

fn task_status_mark(task: &Task, t: &Theme) -> (El, &'static str) {
    if !task.reachable {
        return (ui::dot(t.faint), "offline");
    }
    match task.status {
        TaskStatus::Queued => (
            el().size(S8, S8)
                .radius(S4)
                .border(1.5, t.muted)
                .a11y_hidden(),
            "queued",
        ),
        TaskStatus::Running => (ui::radar(ICON_SMALL, t.accent, t), "running"),
        TaskStatus::Completed => (ui::dot(t.success), "done"),
        TaskStatus::Failed => (ui::dot(t.danger), "failed"),
        TaskStatus::Cancelled => (ui::dot(t.faint), "stopped"),
    }
}

fn task_row_text(
    title: &str,
    remote: bool,
    environment: &str,
    child: Option<&ThreadSummary>,
    task: &Task,
    running: bool,
    t: &Theme,
) -> El {
    let model = child.and_then(model_detail);
    let (preview_text, preview_is_error) = match (
        task.error.as_deref().and_then(first_line),
        task.result.as_deref().and_then(first_line),
    ) {
        (Some(err), _) => (Some(err), true),
        (None, Some(res)) => (Some(res), false),
        (None, None) => (None, false),
    };
    let mut line1 = row().w_full().gap(S6).center_y().child(
        text(title)
            .font(t.ui_medium)
            .color(if running { t.text } else { t.muted })
            .group_color(t.text)
            .shrink()
            .truncate()
            .help_when_truncated(title),
    );
    if remote {
        line1 = line1.child(
            text(format!("@{environment}"))
                .font(t.mono_micro)
                .color(t.worktree)
                .a11y_hidden(),
        );
    }
    let mut line2 = row().w_full().gap(S4).center_y();
    let has_line2 = model.is_some() || preview_text.is_some();
    if let Some(model) = model {
        line2 = line2.child(text(model).font(t.small).color(t.faint).shrink().truncate());
        if preview_text.is_some() {
            line2 = line2.child(text("·").font(t.small).color(t.faint).a11y_hidden());
        }
    }
    if let Some(preview) = preview_text {
        line2 = line2.child(
            text(preview)
                .font(t.small)
                .color(if preview_is_error { t.danger } else { t.muted })
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    }
    col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S2)
        .child(line1)
        .children(has_line2.then_some(line2))
}

fn task_row_trailing(
    s: &Desktop,
    task: &Task,
    running: bool,
    state_label: &'static str,
    t: &Theme,
) -> El {
    let mut trailing = row().gap(S4).center_y();
    if !running {
        trailing = trailing.child(
            text(state_label)
                .font(t.mono_micro)
                .color(if task.status == TaskStatus::Failed {
                    t.danger
                } else {
                    t.faint
                })
                .a11y_hidden(),
        );
    }
    if task.reachable && task.thread.is_some() {
        trailing = trailing.child(
            icon(&ui::ARROW_UP_RIGHT, ICON_SMALL + 1.0)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        );
    }
    if s.connected() && running {
        let id = task.id.clone();
        trailing = trailing.child(
            row()
                .key("stop")
                .role("button")
                .label("Stop task")
                .help("Stop task")
                .size(S24, S24)
                .center()
                .radius(RADIUS_INNER)
                .hover_group()
                .press_scale(0.95)
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.send(Command::CancelTask(id.clone()), tx);
                })
                .child(
                    icon(&ui::STOP, ICON_SMALL - 1.0)
                        .color(t.muted)
                        .group_color(t.danger)
                        .a11y_hidden(),
                ),
        );
    }
    trailing
}

fn stop_all_button(ids: Vec<String>, t: &Theme) -> El {
    row()
        .key("stop-all-tasks")
        .role("button")
        .label("Cancel running tasks")
        .help("Cancel running tasks")
        .size(S24, S24)
        .center()
        .radius(RADIUS_INNER)
        .hover_group()
        .press_scale(0.95)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            for id in &ids {
                s.send(Command::CancelTask(id.clone()), tx);
            }
        })
        .child(
            icon(&ui::STOP, ICON_SMALL - 1.0)
                .color(t.faint)
                .group_color(t.danger)
                .a11y_hidden(),
        )
}

fn child_summary<'a>(s: &'a Desktop, task: &Task) -> Option<&'a ThreadSummary> {
    task.thread
        .filter(|_| s.environment.as_deref() == Some(task.environment.as_str()))
        .and_then(|thread| s.summary(thread))
}

fn model_detail(thread: &ThreadSummary) -> Option<Cow<'_, str>> {
    let thinking = thread.thinking.as_deref().map(ui::thinking_label);
    match (thread.model.as_deref(), thinking) {
        (Some(model), Some(thinking)) => Some(Cow::Owned(format!("{model} · {thinking}"))),
        (Some(model), None) => Some(Cow::Borrowed(model)),
        (None, Some(thinking)) => Some(thinking),
        (None, None) => None,
    }
}

fn first_line(outcome: &str) -> Option<&str> {
    outcome.lines().map(str::trim).find(|line| !line.is_empty())
}
