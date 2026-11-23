//! Individual delegated task rows inside the expanded Tasks Dock.

use std::borrow::Cow;

use rune::{El, col, el, icon, row, text};
use viz_client::protocol::{Task, TaskStatus, ThreadSummary};

use crate::{
    app::Desktop,
    model::{Model, first_non_empty_line, thread_title},
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_INNER, S2, S4, S6, S8, S12, S24, Theme},
    ui,
    update::{ComposerMsg, Msg, SelectionMsg},
};

/// Minimum height of one task row in the expanded dock.
const ROW_MIN_HEIGHT: f32 = 44.0;

/// Renders one row in the expanded Tasks Dock.
pub(super) fn row_view(m: &Model, task: &Task, t: &Theme) -> El {
    let running = matches!(task.status, TaskStatus::Running | TaskStatus::Queued);
    let (mark, state_label) = status_mark(task, t);
    let child = child_summary(m, task);
    let harness = child.map(|thread| (thread.harness.as_str(), thread.harness_name.as_str()));
    let title = child.map_or(task.title.as_str(), thread_title);
    let remote = m.connection.environment.as_deref() != Some(task.environment.as_str());
    let text_col = row_text(title, remote, &task.environment, child, task, running, t);
    let trailing = row_trailing(m, task, running, state_label, t);
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
                s.dispatch(
                    Msg::Selection(SelectionMsg::OpenInEnvironment {
                        environment: environment.clone(),
                        thread: Some(thread),
                    }),
                    tx,
                );
            });
    }
    row_el
}

/// Resolves a task's local child thread summary when running in the active
/// environment.
pub(super) fn child_summary<'a>(m: &'a Model, task: &Task) -> Option<&'a ThreadSummary> {
    task.thread
        .filter(|_| m.connection.environment.as_deref() == Some(task.environment.as_str()))
        .and_then(|thread| m.summary(thread))
}

/// Formats model and thinking metadata for a child thread summary.
pub(super) fn model_detail(thread: &ThreadSummary) -> Option<Cow<'_, str>> {
    let thinking = thread.thinking.as_deref().map(ui::thinking_label);
    match (thread.model.as_deref(), thinking) {
        (Some(model), Some(thinking)) => Some(Cow::Owned(format!("{model} · {thinking}"))),
        (Some(model), None) => Some(Cow::Borrowed(model)),
        (None, Some(thinking)) => Some(thinking),
        (None, None) => None,
    }
}

fn status_mark(task: &Task, t: &Theme) -> (El, &'static str) {
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

fn row_text(
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
        task.error.as_deref().and_then(first_non_empty_line),
        task.result.as_deref().and_then(first_non_empty_line),
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
                .color(t.remote)
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

fn row_trailing(m: &Model, task: &Task, running: bool, state_label: &'static str, t: &Theme) -> El {
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
    if m.connected() && running {
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
                .press_scale(motion::PRESS_SCALE)
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Composer(ComposerMsg::CancelTask(id.clone())), tx);
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
