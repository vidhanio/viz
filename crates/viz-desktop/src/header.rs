//! Thread toolbar and shared task identity formatting.

use std::borrow::Cow;

use rune::{El, col, icon, row, text};
use rune_kit::shell::Toolbar;
use viz_client::protocol::{Task, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, Switch, TITLE_ENTER_DELAY, TITLE_TRAVEL, thread_title},
    sidebar,
    theme::{
        HAIRLINE, ICON, ICON_SMALL, READING_INSET, S2, S8, TOOLBAR_HEIGHT, TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;

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
                    .child(ui::harness_mark(harness, false, t))
                    .child(title_text("New thread", t))
            },
        );
    };
    let thread = s.summary(id);
    let mark = ui::harness_mark(
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
                .help_when_truncated(label)
                .a11y_hidden(),
        )
}
