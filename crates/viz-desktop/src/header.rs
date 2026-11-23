//! Thread toolbar and shared task identity formatting.

use std::borrow::Cow;

use rune::{El, col, icon, row, text};
use rune_kit::shell::Toolbar;
use viz_client::protocol::{Task, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, motion_switch},
    git,
    model::{first_non_empty_line, thread_title},
    motion::{Switch, TITLE_ENTER_DELAY, TRAVEL_TITLE},
    sidebar,
    theme::{HAIRLINE, ICON, ICON_SMALL, READING_INSET, S2, S8, Size, TOOLBAR_PAD, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;

/// Visible chat header height below the window chrome.
pub const CHAT_HEADER_HEIGHT: f32 = 44.0;

/// The toolbar. Its title is keyed per thread inside its own clip, so it
/// moves with the transcript when the thread changes.
pub fn toolbar(s: &Desktop, t: &Theme) -> Toolbar {
    let m = &s.model;
    let switch = motion_switch(m.selection.switch);
    let mut toolbar = Toolbar::new().center(ui::drag_guard("toolbar-drag-guard").pin(
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(0.0),
    ));
    let page = match (m.selection.selected, &m.selection.new_thread) {
        (Some(id), _) => format!("title-{id}"),
        (None, Some(draft)) => format!("title-draft-{}", draft.id),
        (None, None) => "title-none".to_owned(),
    };
    let mut entering = title(s, t)
        .key(page)
        .transition(switch.title_enter(TRAVEL_TITLE));
    if switch != Switch::Cut {
        entering = entering.enter_delay(TITLE_ENTER_DELAY);
    }
    let title = row()
        .key("title-pane")
        .h(CHAT_HEADER_HEIGHT)
        .center_y()
        .min_w(0.0)
        .shrink()
        .clip_rounded(0.0)
        .child_exit(switch.title_exit(TRAVEL_TITLE))
        .child(entering);
    let title = if s.widgets.sidebar_pane.is_open() {
        title.ml(READING_INSET - TOOLBAR_PAD)
    } else {
        toolbar = toolbar.leading(sidebar::new_thread_button(s, Size::Default, t));
        title
    };
    let mut toolbar = toolbar.leading(title);
    if m.selection.selected.is_none() {
        return toolbar;
    }
    toolbar = toolbar.trailing(header_button(
        &ui::TERMINAL,
        "Terminal",
        m.panels.terminal_open,
        crate::update::GitMsg::ToggleTerminal,
        t,
    ));
    if let Some(status) = &m.git.status {
        let diff = row()
            .center_y()
            .gap(S2)
            .children(
                (status.stats.added > 0 || status.stats.removed > 0)
                    .then(|| git::diff_panel::stat(status.stats, t)),
            )
            .child(header_button(
                &ui::FILE_DIFF,
                "Diff",
                m.panels.diff.open,
                crate::update::GitMsg::ToggleDiff,
                t,
            ));
        toolbar = toolbar.trailing(diff);
    }
    if !crate::model::git::inline(m) {
        toolbar = toolbar.trailing(
            header_button(
                &ui::PANEL_RIGHT,
                "Thread Details",
                m.panels.details_open,
                crate::update::GitMsg::ToggleDetails,
                t,
            )
            .key(git::details_card::ANCHOR),
        );
    }
    toolbar
}

fn header_button(
    glyph: &'static rune::Icon,
    label: &str,
    selected: bool,
    msg: crate::update::GitMsg,
    t: &Theme,
) -> El {
    ui::icon_button(
        glyph,
        label,
        rune_kit::IconButtonVariant::Ghost,
        Size::Small,
    )
    .on_press_tx(move |s: &mut Desktop, tx| git::dispatch(s, msg.clone(), tx))
    .build(&t.k)
    .a11y_value(if selected { "Open" } else { "Closed" })
}

fn project_mark(project: &viz_client::protocol::Project, t: &Theme) -> El {
    row()
        .center_y()
        .gap(S8)
        .child(ui::project_monogram(&project.name, 18.0, None, t))
        .child(
            text(&project.name)
                .font(t.ui_medium)
                .color(t.muted)
                .max_w(128.0)
                .truncate(),
        )
        .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint).a11y_hidden())
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
        .and_then(first_non_empty_line)
        .or_else(|| task.result.as_deref().and_then(first_non_empty_line))
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
    let m = &s.model;
    let group = row()
        .key("header-title")
        .min_w(0.0)
        .shrink()
        .gap(S8)
        .center_y();
    let Some(id) = m.selection.selected else {
        return m.selection.new_thread.as_ref().map_or_else(
            || title_text("viz", t),
            |draft| {
                let harness = draft
                    .active_harness(m.harnesses())
                    .map(|runner| (runner.id.as_str(), runner.name.as_str()));
                group
                    .child(ui::harness_mark(harness, false, t))
                    .child(title_text("New thread", t))
            },
        );
    };
    let thread = m.summary(id);
    let mark = m.project().map(|project| project_mark(project, t));
    let title = title_text(m.title(id), t);
    let Some((parent, environment)) = thread
        .and_then(|thread| {
            thread
                .parent
                .map(|parent| (parent, thread.parent_environment.as_deref()))
        })
        .filter(|(parent, env)| {
            *parent != id || env.is_some_and(|env| Some(env) != m.connection.environment.as_deref())
        })
    else {
        return group.children(mark).child(title);
    };
    group
        .child(breadcrumb(s, parent, environment, t))
        .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint).a11y_hidden())
        .children(mark)
        .child(title)
}

/// The link back to a subagent's parent: its harness mark and title, or the
/// parent's environment name when it runs elsewhere.
fn breadcrumb(s: &Desktop, parent: ThreadId, environment: Option<&str>, t: &Theme) -> El {
    let m = &s.model;
    let remote = environment.filter(|env| Some(*env) != m.connection.environment.as_deref());
    let label = remote.unwrap_or_else(|| m.title(parent)).to_owned();
    let mark = remote.is_none().then(|| {
        ui::harness_mark(
            m.summary(parent)
                .map(|thread| (thread.harness.as_str(), thread.harness_name.as_str())),
            false,
            t,
        )
    });
    let target = remote.map(str::to_owned);
    row()
        .key("header-parent")
        .role("link")
        .label(format!("Back to {label}"))
        .min_w(0.0)
        .max_w(BREADCRUMB_MAX)
        .shrink()
        .gap(S8)
        .center_y()
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::OpenThreadLink {
                    thread: parent,
                    environment: target.clone(),
                }),
                tx,
            );
        })
        .children(mark)
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
