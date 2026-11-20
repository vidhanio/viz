//! Thread toolbar and the persistent task panel.

use rune::{El, ScrollSpec, Transition, col, el, icon, row, text};
use rune_kit::{ProgressCircle, shell::Toolbar};
use viz_client::protocol::{Task, TaskStatus, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, thread_title},
    backend::Command,
    sidebar,
    theme::{
        CARD_WIDTH, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, READING_INSET, S2, S4, S8, S12, S16,
        S32, S40, Size, TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;
/// Leave at least this much for the thread before putting the panel beside it.
const MIN_THREAD_WIDTH: f32 = 560.0;
const PANEL_GAP: f32 = S16;
const PREVIOUS: &str = "agents-previous";
const COMPACT_ROWS: f32 = 2.0;
const WIDE_ROWS: f32 = 6.0;

pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    // First in the band, under the title and buttons.
    let mut toolbar = Toolbar::new().center(ui::drag_guard("toolbar-drag-guard").pin(
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.0),
    ));
    let title = if s.sidebar.is_open() {
        title(s, t).ml(READING_INSET - TOOLBAR_PAD)
    } else {
        toolbar = toolbar.leading(sidebar::new_thread_button(s, t));
        title(s, t)
    };
    toolbar
        .leading(title)
        .trailing(metadata(s, t).mr(READING_INSET - TOOLBAR_PAD))
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
        .max_h(if wide { WIDE_ROWS } else { COMPACT_ROWS } * S40)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
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
    let k = &t.overlay;
    let panel = k.shadow_layers(&k.elevation.floating).fold(
        col()
            .key(format!("agents-card-{parent}"))
            .role("group")
            .label("Tasks")
            .w(CARD_WIDTH)
            .radius(RADIUS_CARD)
            .fill(k.c.layer_01)
            .transition(Transition::opacity())
            .child(header)
            .child(list),
        El::box_shadow,
    );
    if wide {
        (
            Some(panel.pin(None, Some(S12), Some(READING_INSET), None)),
            CARD_WIDTH + PANEL_GAP,
        )
    } else {
        let band = row()
            .key("agents-band")
            .w_full()
            .pt(S8)
            .pr(READING_INSET)
            .child(el().grow(1.0).a11y_hidden())
            .child(panel);
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
        .hover(t.hover)
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
            .color(t.faint),
        )
        .child(ui::section("Previous tasks", t).grow(1.0))
        .child(text(count.to_string()).font(t.mono_small).color(t.faint))
}

/// The same identity block in the panel and the timeline. Use reported model
/// and effort, never the defaults for that harness.
pub fn agent_identity(thread: &ThreadSummary, t: &Theme) -> El {
    let label = thread_title(thread);
    let thinking = thread.thinking.as_deref().map(ui::thinking_label);
    let detail = match (thread.model.as_deref(), thinking) {
        (Some(model), Some(thinking)) => Some(format!("{model} · {thinking}")),
        (Some(model), None) => Some(model.to_owned()),
        (None, Some(thinking)) => Some(thinking.into_owned()),
        (None, None) => Some(thread.harness.clone()),
    };
    row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .center_y()
        .gap(S8)
        .child(
            icon(ui::harness_icon(&thread.harness), ICON)
                .color(t.muted)
                .a11y_hidden(),
        )
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
                .children(detail.map(|detail| {
                    text(detail.clone())
                        .font(t.small)
                        .color(t.muted)
                        .truncate()
                        .help_when_truncated(detail)
                })),
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
            TaskStatus::Cancelled => (ui::dot(t.faint), "Cancelled"),
        }
    } else {
        (ui::dot(t.faint), "Unreachable")
    };
    let child = task
        .thread
        .filter(|_| desktop.environment.as_deref() == Some(task.environment.as_str()))
        .and_then(|thread| desktop.summary(thread));
    let mut detail = child.map_or_else(
        || {
            row()
                .grow(1.0)
                .min_w(0.0)
                .center_y()
                .gap(S8)
                .child(icon(&ui::AGENT, ICON).color(t.muted).a11y_hidden())
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .child(text(&task.title).font(t.ui_medium).color(t.text).truncate())
                        .child(
                            text(&task.environment)
                                .font(t.small)
                                .color(t.muted)
                                .truncate(),
                        ),
                )
        },
        |thread| agent_identity(thread, t),
    );
    if task.reachable
        && let Some(thread) = task.thread
    {
        let environment = task.environment.clone();
        detail = detail
            .role("link")
            .label(task.title.clone())
            .focusable(true)
            .hover(t.hover)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.open_in_environment(environment.clone(), Some(thread), tx);
            });
    }
    let mut row = row()
        .key(format!("task-{}", task.id))
        .description(state)
        .help(
            task.error
                .as_deref()
                .or(task.result.as_deref())
                .unwrap_or(state),
        )
        .h(S40)
        .px(S8)
        .gap(S8)
        .center_y()
        .child(detail)
        .child(mark);
    if desktop.connected() && matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
        let id = task.id.clone();
        row = row.child(
            ui::icon_button(
                &ui::STOP,
                "Cancel task",
                IconButtonVariant::GhostMuted,
                Size::Small,
            )
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.send(Command::CancelTask(id.clone()), tx);
            })
            .build(&t.k),
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
                    .map(|runner| runner.name.as_str());
                group
                    .child(harness_mark(harness, t))
                    .child(title_text("New thread", t))
            },
        );
    };
    let thread = s.summary(id);
    let mark = harness_mark(thread.map(|thread| thread.harness.as_str()), t);
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

fn harness_mark(harness: Option<&str>, t: &Theme) -> El {
    let mark = icon(harness.map_or(&ui::HARNESS, ui::harness_icon), ICON).color(t.muted);
    match harness {
        Some(name) => mark.help(name),
        None => mark.a11y_hidden(),
    }
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

fn metadata(s: &Desktop, t: &Theme) -> El {
    let mut metadata = row().key("header-metadata").gap(S8).center_y();
    if let Some(state) = &s.state {
        if let Some(usage) = &state.usage
            && usage.size > 0
        {
            let fraction = crate::theme::fraction(usage.used, usage.size);
            let percent = format!("{:.0}%", fraction * 100.0);
            let label = format!(
                "Context: {} / {} tokens ({percent})",
                usage.used, usage.size
            );
            metadata = metadata.child(
                row()
                    .center_y()
                    .gap(S4)
                    .label(label.clone())
                    .help(label)
                    .child(
                        ProgressCircle::new(fraction)
                            .size(ICON_SMALL)
                            .build(&t.k)
                            .a11y_hidden(),
                    )
                    .child(text(percent).font(t.small).color(t.muted).a11y_hidden()),
            );
        }
        if let Some(mark) = ui::status(state.status, t) {
            metadata = metadata.child(mark.key("header-status"));
        }
    }
    metadata
}
