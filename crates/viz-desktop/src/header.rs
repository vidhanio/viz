//! Thread toolbar and the persistent task panel.

use std::borrow::Cow;

use rune::{El, ScrollSpec, Transition, col, icon, row, text};
use rune_kit::shell::Toolbar;
use viz_client::protocol::{Task, TaskStatus, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, Switch, TITLE_ENTER_DELAY, TITLE_TRAVEL, thread_title},
    server::Command,
    sidebar,
    theme::{
        CARD_WIDTH, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, READING_INSET, S2, S4, S6, S8, S12,
        S16, S32, S40, Size, TOOLBAR_HEIGHT, TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;
/// Leave at least this much for the thread before putting the panel beside it.
const MIN_THREAD_WIDTH: f32 = 560.0;
const PANEL_GAP: f32 = S16;
const PREVIOUS: &str = "agents-previous";
/// Tallest the task list gets in narrow windows (fits two multi-line rows
/// without clipping).
const COMPACT_HEIGHT: f32 = S40 * 4.0;
/// Tallest the task list gets beside the transcript in wide windows.
const WIDE_HEIGHT: f32 = S40 * 7.0;

/// The toolbar. Its title is keyed per thread inside its own clip, so it
/// moves with the transcript when the thread changes (`app::Switch`).
pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    let mut toolbar = Toolbar::new().center(ui::drag_guard("toolbar-drag-guard").pin(
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.0),
    ));
    let page = match (s.selected, &s.new_thread) {
        (Some(id), _) => format!("title-{id}"),
        (None, Some(draft)) => format!("title-draft-{}", draft.id),
        (None, None) => "title-none".to_owned(),
    };
    let mut entering = title(s, t)
        .key(page)
        .transition(s.switch.title_enter(TITLE_TRAVEL));
    if s.switch != Switch::Cut {
        entering = entering.enter_delay(TITLE_ENTER_DELAY);
    }
    let title = row()
        .key("title-pane")
        .h(TOOLBAR_HEIGHT)
        .center_y()
        .min_w(0.0)
        .shrink()
        .clip_rounded(0.0)
        .child_exit(s.switch.title_exit(TITLE_TRAVEL))
        .child(entering);
    let title = if s.sidebar.is_open() {
        title.ml(READING_INSET - TOOLBAR_PAD)
    } else {
        toolbar = toolbar.leading(sidebar::new_thread_button(s, t));
        title
    };
    toolbar.leading(title)
}

/// The panel and its horizontal reserve. In small windows it gets its own
/// band above the transcript instead of covering the latest message.
pub fn agents(s: &Desktop, t: &Theme) -> (Option<El>, f32) {
    let Some(parent) = s.selected else {
        return (None, 0.0);
    };
    let children: Vec<_> = s
        .tasks
        .iter()
        .filter(|task| task.parent == parent)
        .collect();
    if children.is_empty() {
        return (None, 0.0);
    }
    let wide =
        s.window_width - s.sidebar.shown_width() >= MIN_THREAD_WIDTH + CARD_WIDTH + PANEL_GAP;
    let (running, previous): (Vec<_>, Vec<_>) = children
        .into_iter()
        .partition(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Queued));
    let mut header = row()
        .h(S32)
        .px(S12)
        .gap(S8)
        .center_y()
        .child(icon(&ui::AGENT, ICON_SMALL).color(t.muted).a11y_hidden())
        .child(ui::section("Tasks", t).grow(1.0));
    if !running.is_empty() {
        header = header.child(
            text(format!("{} running", running.len()))
                .font(t.small)
                .color(t.muted),
        );
    }
    let mut list = col()
        .key("subagent-list")
        .w_full()
        .px(S4)
        .pb(S4)
        .max_h(if wide { WIDE_HEIGHT } else { COMPACT_HEIGHT })
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S16)
        .children(running.into_iter().map(|thread| task_row(thread, s, t)));
    if previous.len() == 1 {
        list = list.child(task_row(previous[0], s, t));
    } else if !previous.is_empty() {
        let shown = s.expanded.contains(PREVIOUS);
        list = list.child(previous_toggle(previous.len(), shown, t));
        if shown {
            list = list.children(previous.into_iter().map(|thread| task_row(thread, s, t)));
        }
    }
    let mut panel = col()
        .key(format!("agents-card-{parent}"))
        .role("group")
        .label("Tasks")
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .transition(Transition::opacity())
        .child(header)
        .child(list);
    if wide {
        panel = panel.w(CARD_WIDTH);
        (
            Some(panel.pin(None, Some(S12), Some(READING_INSET), None)),
            CARD_WIDTH + PANEL_GAP,
        )
    } else {
        let band = ui::reading(panel.w_full()).key("agents-band").pt(S12);
        (Some(band), 0.0)
    }
}

fn previous_toggle(count: usize, shown: bool, t: &Theme) -> El {
    row()
        .key(PREVIOUS)
        .role("button")
        .label("Previous tasks")
        .expanded(shown)
        .h(S32)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| s.toggle(PREVIOUS))
        .child(
            icon(
                if shown {
                    &ui::CHEVRON_OPEN
                } else {
                    &ui::CHEVRON
                },
                ICON_SMALL,
            )
            .color(t.faint)
            .group_color(t.text),
        )
        .child(
            ui::section("Previous tasks", t)
                .group_color(t.text)
                .grow(1.0),
        )
        .child(
            text(count.to_string())
                .font(t.mono_small)
                .color(t.faint)
                .group_color(t.text),
        )
}

/// Identity for older timeline entries that have no durable task.
pub fn agent_identity(thread: &ThreadSummary, t: &Theme) -> El {
    thread_identity(thread, None, t)
}

/// Identity shared by durable task rows in the panel and the timeline.
pub fn task_identity(task: &Task, child: Option<&ThreadSummary>, t: &Theme) -> El {
    let preview = task
        .error
        .as_deref()
        .and_then(first_line)
        .or_else(|| task.result.as_deref().and_then(first_line))
        .map(|line| text(line).font(t.small).color(t.muted).truncate());
    if let Some(thread) = child {
        thread_identity(thread, preview, t)
    } else {
        identity(
            &task.title,
            &task.environment,
            icon(&ui::AGENT, ICON)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
            preview,
            t,
        )
    }
}

fn first_line(outcome: &str) -> Option<&str> {
    outcome.lines().map(str::trim).find(|line| !line.is_empty())
}

fn thread_identity(thread: &ThreadSummary, preview: Option<El>, t: &Theme) -> El {
    let label = thread_title(thread);
    let thinking = thread.thinking.as_deref().map(ui::thinking_label);
    let detail = match (thread.model.as_deref(), thinking) {
        (Some(model), Some(thinking)) => Cow::Owned(format!("{model} · {thinking}")),
        (Some(model), None) => Cow::Borrowed(model),
        (None, Some(thinking)) => thinking,
        (None, None) => Cow::Borrowed(thread.harness_name.as_str()),
    };
    identity(
        label,
        &detail,
        icon(ui::harness_icon(&thread.harness), ICON)
            .color(t.muted)
            .group_color(t.text)
            .help(&thread.harness_name)
            .a11y_hidden(),
        preview,
        t,
    )
}

fn identity(label: &str, detail: &str, avatar: El, preview: Option<El>, t: &Theme) -> El {
    row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .gap(S8)
        .child(avatar)
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(
                    text(label)
                        .font(t.ui_medium)
                        .color(t.text)
                        .truncate()
                        .help_when_truncated(label),
                )
                .child(
                    text(detail)
                        .font(t.small)
                        .color(t.muted)
                        .group_color(t.text)
                        .truncate()
                        .help_when_truncated(detail),
                )
                .children(preview),
        )
}

fn task_row(task: &Task, desktop: &Desktop, t: &Theme) -> El {
    use rune_kit::IconButtonVariant;

    let (mark, state) = if task.reachable {
        match task.status {
            TaskStatus::Queued => (ui::dot(t.muted), "Queued"),
            TaskStatus::Running => (ui::spinner(t), "Running"),
            TaskStatus::Completed => (ui::dot(t.success), "Done"),
            TaskStatus::Failed => (ui::dot(t.danger), "Failed"),
            TaskStatus::Cancelled => (ui::dot(t.faint), "Stopped"),
        }
    } else {
        (ui::dot(t.faint), "Unreachable")
    };
    let child = task
        .thread
        .filter(|_| desktop.environment.as_deref() == Some(task.environment.as_str()))
        .and_then(|thread| desktop.summary(thread));
    let detail = task_identity(task, child, t);
    let mut row = row()
        .key(format!("task-{}", task.id))
        .description(state)
        .help(
            task.error
                .as_deref()
                .or(task.result.as_deref())
                .unwrap_or(state),
        )
        .w_full()
        .min_h(S40)
        .py(S6)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(detail)
        .child(mark);
    if task.reachable
        && let Some(thread) = task.thread
    {
        let environment = task.environment.clone();
        row = row
            .role("link")
            .label(task.title.clone())
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .hover_group()
            .press(t.k.c.pressed)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.open_in_environment(environment.clone(), Some(thread), tx);
            });
    }
    if desktop.connected() && matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
        let id = task.id.clone();
        row = row.child(
            ui::icon_button(
                &ui::STOP,
                "Stop task",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.send(Command::CancelTask(id.clone()), tx);
            })
            .build(&t.k)
            .hover_alone(),
        );
    }
    row
}

fn title_text(label: &str, t: &Theme) -> El {
    text(label)
        .key("title")
        .font(t.title)
        .color(t.text)
        .min_w(0.0)
        .truncate()
        .shrink()
        .help_when_truncated(label)
}

fn title(s: &Desktop, t: &Theme) -> El {
    let group = row()
        .key("header-title")
        .min_w(0.0)
        .shrink()
        .gap(S8)
        .center_y();
    let Some(id) = s.selected else {
        return s.new_thread.as_ref().map_or_else(
            || title_text("viz", t),
            |draft| {
                let harness = draft
                    .active_harness(&s.harnesses)
                    .map(|runner| (runner.id.as_str(), runner.name.as_str()));
                group
                    .child(ui::medallion(harness, false, t))
                    .child(title_text("New thread", t))
            },
        );
    };
    let thread = s.summary(id);
    let mark = ui::medallion(
        thread.map(|thread| (thread.harness.as_str(), thread.harness_name.as_str())),
        s.busy(),
        t,
    );
    let title = title_text(s.title(id), t);
    let Some((parent, environment)) = thread
        .and_then(|thread| {
            thread
                .parent
                .map(|parent| (parent, thread.parent_environment.as_deref()))
        })
        .filter(|(parent, env)| {
            *parent != id || env.is_some_and(|env| Some(env) != s.environment.as_deref())
        })
    else {
        return group.child(mark).child(title);
    };
    group
        .child(breadcrumb(s, parent, environment, t))
        .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint).a11y_hidden())
        .child(mark)
        .child(title)
}

fn breadcrumb(s: &Desktop, parent: ThreadId, environment: Option<&str>, t: &Theme) -> El {
    let remote = environment.filter(|env| Some(*env) != s.environment.as_deref());
    let label = remote.unwrap_or_else(|| s.title(parent)).to_owned();
    let target = remote.map(str::to_owned);
    row()
        .key("header-parent")
        .role("link")
        .label(format!("Back to {label}"))
        .min_w(0.0)
        .max_w(BREADCRUMB_MAX)
        .shrink()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            if let Some(env) = &target {
                s.open_in_environment(env.clone(), Some(parent), tx);
            } else {
                s.select(parent, tx);
            }
        })
        .child(
            text(&label)
                .font(t.title)
                .color(t.muted)
                .group_color(t.text)
                .min_w(0.0)
                .truncate()
                .shrink()
                .help(label)
                .a11y_hidden(),
        )
}
