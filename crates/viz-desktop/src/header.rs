//! Thread toolbar and shared task identity formatting.

use std::borrow::Cow;

use rune::{El, col, icon, row, text};
use rune_kit::{Menu, MenuItem, Placement, shell::Toolbar};
use viz_client::protocol::{GitSummary, Task, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, Switch, TITLE_ENTER_DELAY, TITLE_TRAVEL, thread_title},
    sidebar,
    theme::{
        HAIRLINE, ICON, ICON_SMALL, RADIUS_CONTROL, READING_INSET, S2, S4, S6, S8, Size,
        TOOLBAR_HEIGHT, TOOLBAR_PAD, Theme,
    },
    ui,
};

/// The widest the parent's title gets in a subagent's breadcrumb.
const BREADCRUMB_MAX: f32 = 272.0;
/// Maximum width of the branch label in the right-side branch toolbar.
const BRANCH_LABEL_MAX: f32 = 148.0;
/// `open_picker` key for the thread header's branch popover.
const HEADER_BRANCH_PICKER: &str = "header-branch";

/// The toolbar. Its title is keyed per thread inside its own clip, so it
/// moves with the transcript when the thread changes (`app::Switch`).
pub fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Toolbar {
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
    toolbar = toolbar.leading(title);
    if let Some(controls) = git_toolbar(s, t, layer) {
        toolbar = toolbar.trailing(controls);
    }
    toolbar
}

/// Right-aligned Git branch status popover and diff panel toggle for the open
/// thread when it sits in a Git repository.
fn git_toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let id = s.selected?;
    let summary = s.summary(id)?;
    let git = summary.git.as_ref()?;
    let branch_trigger = branch_menu(s, summary, git, t, layer);
    let diff_toggle = diff_toggle_button(s, git, t);
    Some(
        row()
            .key("header-git-toolbar")
            .gap(S6)
            .center_y()
            .child(branch_trigger)
            .child(diff_toggle),
    )
}

fn branch_menu(
    s: &Desktop,
    summary: &ThreadSummary,
    git: &GitSummary,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let linked = summary.worktree.is_some() || git.linked_worktree;
    let branch_label = summary
        .worktree
        .as_ref()
        .map(|wt| wt.branch.as_str())
        .or(git.branch.as_deref())
        .unwrap_or("detached");
    let open = s.open_picker.as_deref() == Some(HEADER_BRANCH_PICKER);
    let glyph = if linked {
        &ui::FOLDER_GIT_2
    } else {
        &ui::GIT_BRANCH
    };
    let icon_color = if linked { t.worktree } else { t.muted };
    let trigger = row()
        .key("header-branch-trigger")
        .role("button")
        .label(format!("Branch {branch_label}"))
        .expanded(open)
        .help(if linked {
            "Linked Git worktree details"
        } else {
            "Git branch details"
        })
        .h(Size::Default.height())
        .px(S8)
        .gap(S4)
        .center_y()
        .radius(RADIUS_CONTROL)
        .continuous_corners()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(0.97)
        .on_press(|s: &mut Desktop| {
            if s.open_picker.as_deref() == Some(HEADER_BRANCH_PICKER) {
                s.open_picker = None;
            } else {
                s.open_picker = Some(HEADER_BRANCH_PICKER.to_owned());
            }
        })
        .child(
            icon(glyph, ICON_SMALL)
                .color(icon_color)
                .group_color(if linked { t.worktree } else { t.text })
                .a11y_hidden(),
        )
        .child(
            text(branch_label.to_owned())
                .font(t.mono_small)
                .color(if linked { t.worktree } else { t.muted })
                .group_color(if linked { t.worktree } else { t.text })
                .max_w(BRANCH_LABEL_MAX)
                .truncate()
                .a11y_hidden(),
        )
        .children((git.dirty_files > 0).then(|| {
            text(format!("•{}", git.dirty_files))
                .font(t.mono_micro)
                .color(t.warning)
                .a11y_hidden()
        }));
    let mut menu = Menu::new("header-branch-menu")
        .placement(Placement::BottomEnd)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.open_picker = None)
        .trigger(trigger);
    if open {
        menu = populate_branch_menu(menu, s, summary, git, linked);
    }
    menu.build(&t.overlay, layer)
}

fn populate_branch_menu(
    mut menu: Menu,
    s: &Desktop,
    summary: &ThreadSummary,
    git: &GitSummary,
    linked: bool,
) -> Menu {
    let checkout = summary
        .worktree
        .as_ref()
        .map(|wt| wt.path.clone())
        .or_else(|| s.state.as_ref().map(|st| st.cwd.clone()));
    let mut status_bits = vec![if linked {
        "Linked worktree".to_owned()
    } else {
        "Local checkout".to_owned()
    }];
    if let Some(ahead) = git.ahead.filter(|&n| n > 0) {
        status_bits.push(format!("↑{ahead}"));
    }
    if let Some(behind) = git.behind.filter(|&n| n > 0) {
        status_bits.push(format!("↓{behind}"));
    }
    if git.dirty_files > 0 {
        status_bits.push(format!("{} modified", git.dirty_files));
    }
    menu = menu.item(MenuItem::new(status_bits.join(" · ")).disabled(true));
    if let Some(path) = checkout {
        menu = menu.item(
            MenuItem::new("Copy checkout path")
                .icon(&ui::COPY)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.open_picker = None;
                    tx.copy(path.clone());
                    s.toast(tx, "Copied checkout path", None);
                }),
        );
    }
    menu.separator().item(
        MenuItem::new("New worktree thread")
            .icon(&ui::GIT_BRANCH_PLUS)
            .disabled(!s.connected())
            .on_select_tx(|s: &mut Desktop, tx| {
                s.open_picker = None;
                s.create_in_worktree(tx);
            }),
    )
}

fn diff_toggle_button(s: &Desktop, git: &GitSummary, t: &Theme) -> El {
    let active = s.diff.open;
    let label = if git.dirty_files > 0 {
        format!(
            "{} {}",
            git.dirty_files,
            if git.dirty_files == 1 {
                "file"
            } else {
                "files"
            }
        )
    } else {
        "Diff".to_owned()
    };
    let mut chip = row()
        .key("header-diff-toggle")
        .role("button")
        .label("Toggle diff panel")
        .expanded(active)
        .help(format!("Toggle diff panel ({})", ui::shortcut("D")))
        .h(Size::Default.height())
        .px(S8)
        .gap(S4)
        .center_y()
        .radius(RADIUS_CONTROL)
        .continuous_corners()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(0.97)
        .on_press_tx(Desktop::toggle_diff_panel);
    if active {
        chip = chip
            .fill(t.accent_veil)
            .border(HAIRLINE, t.accent.alpha(0.35));
    }
    chip.child(
        icon(&ui::FILE_DIFF, ICON_SMALL)
            .color(if active { t.accent_text } else { t.muted })
            .group_color(if active { t.accent_text } else { t.text })
            .a11y_hidden(),
    )
    .child(
        text(label)
            .font(t.small_medium)
            .color(if active { t.accent_text } else { t.muted })
            .group_color(if active { t.accent_text } else { t.text })
            .a11y_hidden(),
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
