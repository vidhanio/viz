//! Sidebar thread rows, delegated child subtrees, and project group rows.

use std::borrow::Cow;

use rune::{Color, El, LineCap, Shape, canvas, col, el, icon, row, text};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{Project, TaskStatus, ThreadStatus, ThreadSummary};

use super::actions;
use crate::{
    app::Desktop,
    model::{SidebarOutline, thread_title},
    theme::{
        self, HAIRLINE, ICON, ICON_SMALL, RADIUS_CONTROL, RADIUS_INNER, ROW_INSET, S2, S4, S6,
        Size, TREE_ROW_CHILD, TREE_STEP, Theme,
    },
    ui::{self, TreeLine},
    update::{Msg, SelectionMsg, SidebarMsg},
};

/// Renders a top-level thread row and any expanded delegated child rows.
pub(super) fn thread_row(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    thread: &ThreadSummary,
    ink: Color,
    detail: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let id = thread.id;
    let selected = outline.open == Some(id);
    let key = format!("thread-{id}");
    let menu_open = actions::menu_open(s, &key);
    let kids = outline.children.get(&id).map_or(&[][..], Vec::as_slice);
    let auto_open = !outline.query.is_empty() || outline.has_active_or_selected_child(id);
    let show_kids = !kids.is_empty() && (auto_open || !m.sidebar.collapsed_parents.contains(&id));
    let rest = thread_rest_slot(outline, thread, selected, kids.len(), show_kids, t);
    let menu = actions::thread_menu(s, thread, &key, t, layer);
    let branch = thread
        .worktree
        .as_ref()
        .map(|worktree| worktree.branch.as_deref().unwrap_or("detached"));
    let place = if detail {
        let project = thread
            .project
            .as_deref()
            .and_then(|id| m.cache().project(id))
            .map_or("No project", |project| project.name.as_str());
        Some(branch.map_or(Cow::Borrowed(project), |branch| {
            Cow::Owned(format!("{project} · {branch}"))
        }))
    } else {
        branch.map(Cow::Borrowed)
    };
    let mark = {
        let harness = &thread.harness;
        let ink = if thread.lifecycle.archived {
            t.faint
        } else if selected {
            t.text
        } else {
            ink
        };
        icon(ui::harness_icon(harness), ICON)
            .color(ink)
            .group_color(if thread.lifecycle.archived {
                t.muted
            } else {
                t.text
            })
            .a11y_hidden()
    };
    let mut node = TreeLine {
        key: &key,
        label: thread_title(thread),
        detail: place.as_deref(),
        depth: 0,
        strong: thread.lifecycle.unread && !thread.lifecycle.archived,
        dim: thread.lifecycle.archived,
        selected,
        ink,
    }
    .build(Some(mark), ui::trailing(rest, Some(menu), menu_open), t)
    .on_press_tx(move |s: &mut Desktop, tx| {
        s.dispatch(Msg::Selection(SelectionMsg::Select(id)), tx);
    });
    if thread.lifecycle.unread {
        node = node.description("Unread");
    }
    if m.connected() {
        let menu_key = key;
        node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
            s.dispatch_local(Msg::Sidebar(SidebarMsg::OpenRowMenuAt {
                key: menu_key.clone(),
                x: at.x,
                y: at.y,
            }));
        });
    }
    if kids.is_empty() {
        return node;
    }
    let mut branch = col()
        .key(format!("thread-branch-{id}"))
        .w_full()
        .child(node);
    if show_kids {
        let active_tree = outline.has_active_or_selected_child(id);
        branch = branch.children(child_rows(s, outline, kids, 1, active_tree, t));
    }
    branch
}

fn thread_rest_slot(
    outline: &SidebarOutline<'_>,
    thread: &ThreadSummary,
    selected: bool,
    kid_count: usize,
    show_kids: bool,
    t: &Theme,
) -> Option<El> {
    let id = thread.id;
    ui::status(thread.status, selected, t)
        .or_else(|| {
            thread.lifecycle.unread.then(|| {
                ui::dot(t.accent)
                    .role("status")
                    .label("Unread")
                    .help("Unread")
            })
        })
        .or_else(|| {
            (kid_count > 0 && !show_kids).then(|| {
                row()
                    .key("child-count")
                    .role("button")
                    .label(format!("Show {kid_count} delegated tasks"))
                    .help("Expand delegated tasks")
                    .px(S4)
                    .h(18.0)
                    .center()
                    .radius(RADIUS_INNER)
                    .on_press(move |s: &mut Desktop| {
                        s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleParent(id)));
                    })
                    .child(
                        text(kid_count.to_string())
                            .font(t.mono_micro)
                            .color(t.delegated),
                    )
            })
        })
        .or_else(|| {
            theme::relative_age(thread.updated_at, outline.now)
                .map(|age| text(age).font(t.mono_micro).color(t.faint).a11y_hidden())
        })
}

fn child_rows<'a>(
    s: &Desktop,
    outline: &SidebarOutline<'a>,
    kids: &[&'a ThreadSummary],
    depth: u16,
    active_tree: bool,
    t: &Theme,
) -> Vec<El> {
    let mut out = Vec::new();
    let last_index = kids.len().saturating_sub(1);
    for (index, child) in kids.iter().enumerate() {
        let last = index == last_index;
        out.push(child_row(s, outline, child, depth, last, active_tree, t));
        if let Some(grandchildren) = outline.children.get(&child.id) {
            let grand_active = outline.has_active_or_selected_child(child.id);
            let show_grand = !outline.query.is_empty()
                || grand_active
                || !s.model.sidebar.collapsed_parents.contains(&child.id);
            if show_grand {
                out.extend(child_rows(
                    s,
                    outline,
                    grandchildren,
                    depth + 1,
                    grand_active,
                    t,
                ));
            }
        }
    }
    out
}

fn child_row(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    child: &ThreadSummary,
    depth: u16,
    last: bool,
    active_tree: bool,
    t: &Theme,
) -> El {
    let id = child.id;
    let selected = outline.open == Some(id);
    let elbow = child_guide(last, active_tree || selected, t);
    let task_status = s
        .model
        .tasks()
        .iter()
        .find(|task| task.thread == Some(id))
        .map(|task| task.status);
    let status_mark = match (child.status, task_status) {
        (ThreadStatus::Working, _) | (_, Some(TaskStatus::Running)) => Some(ui::radar(
            11.0,
            if selected { t.accent } else { t.muted },
            t,
        )),
        (ThreadStatus::Failed, _) | (_, Some(TaskStatus::Failed)) => Some(ui::dot(t.danger)),
        (_, Some(TaskStatus::Completed)) => Some(ui::dot(t.success)),
        (_, Some(TaskStatus::Cancelled)) => Some(ui::dot(t.faint)),
        _ => None,
    };
    let title = thread_title(child);
    let indent = f32::from(depth.saturating_sub(1)).mul_add(TREE_STEP, ROW_INSET);
    row()
        .key(format!("thread-{id}"))
        .role("treeitem")
        .label(title)
        .level(u32::from(depth) + 1)
        .w_full()
        .h(TREE_ROW_CHILD)
        .pl(indent)
        .pr(S6)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if selected {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Selection(SelectionMsg::Select(id)), tx);
        })
        .child(elbow)
        .child(
            icon(ui::harness_icon(&child.harness), ICON_SMALL + 1.0)
                .color(if selected { t.text } else { t.faint })
                .group_color(t.muted)
                .a11y_hidden(),
        )
        .child(
            text(title)
                .font(if selected { t.small_medium } else { t.small })
                .color(if selected { t.text } else { t.muted })
                .group_color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .help_when_truncated(title)
                .a11y_hidden(),
        )
        .children(status_mark.map(|mark| el().size(ICON, ICON).center().child(mark)))
}

/// Renders a project group header and its threads when expanded.
pub(super) fn project_row(
    s: &Desktop,
    outline: &SidebarOutline<'_>,
    project: &Project,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let open = !outline.query.is_empty() || !m.sidebar.collapsed_projects.contains(&project.path);
    let id = project.path.clone();
    let new_id = project.path.clone();
    let key = format!("project-{id}");
    let menu_open = actions::menu_open(s, &key);
    let leading = if open {
        let layer_of = |glyph: &'static rune::Icon, rest: f32, hovered: f32| {
            el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .center()
                .group_opacity(rest, hovered)
                .child(icon(glyph, ICON).color(t.muted).group_color(t.text))
        };
        el().size(ICON, ICON)
            .child(layer_of(&ui::FOLDER, 1.0, 0.0))
            .child(layer_of(&ui::CHEVRON_OPEN, 0.0, 1.0))
    } else {
        el().size(ICON, ICON)
            .center()
            .child(icon(&ui::CHEVRON, ICON).color(t.faint).group_color(t.text))
    };
    let add_thread = ui::icon_button(
        &ui::PLUS,
        &format!("New thread in {}", project.name),
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .disabled(!m.connected() || m.selection.new_thread.is_some())
    .on_press_tx(move |s: &mut Desktop, tx| {
        s.dispatch(
            Msg::Selection(SelectionMsg::CreateInProject(new_id.clone())),
            tx,
        );
    })
    .build(&t.k)
    .key("project-new-thread");
    let menu = actions::project_menu(s, project, &key, t, layer);
    let trailing_actions = row()
        .gap(S2)
        .center_y()
        .group_opacity(if menu_open { 1.0 } else { 0.0 }, 1.0)
        .child(add_thread)
        .child(menu);
    let mut node = TreeLine {
        key: &key,
        label: &project.name,
        detail: None,
        depth: 0,
        strong: true,
        dim: false,
        selected: false,
        ink: t.text,
    }
    .build(Some(leading), trailing_actions, t)
    .expanded(open)
    .help(&project.path)
    .on_press(move |s: &mut Desktop| {
        s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleProject(id.clone())));
    });
    if m.connected() {
        let menu_key = key;
        node = node.on_secondary_press(move |s: &mut Desktop, at, _| {
            s.dispatch_local(Msg::Sidebar(SidebarMsg::OpenRowMenuAt {
                key: menu_key.clone(),
                x: at.x,
                y: at.y,
            }));
        });
    }
    let mut branch = col()
        .key(format!("project-branch-{}", project.path))
        .w_full()
        .child(node);
    if let Some(threads) = outline.projects.get(project.path.as_str()).filter(|_| open) {
        branch = branch.children(
            threads
                .iter()
                .map(|thread| thread_row(s, outline, thread, t.muted, false, t, layer).level(2)),
        );
    }
    branch
}

/// Vector L-elbow (last child) or T-junction (middle child) tree guide.
fn child_guide(last: bool, highlighted: bool, t: &Theme) -> El {
    let guide = if highlighted {
        t.delegated.alpha(0.5)
    } else {
        t.hairline
    };
    let shape = if last {
        Shape::new().stroke(
            vec![
                [7.0, 0.0],
                [7.0, TREE_ROW_CHILD / 2.0],
                [TREE_STEP - S4, TREE_ROW_CHILD / 2.0],
            ],
            HAIRLINE,
            guide,
        )
    } else {
        Shape::new()
            .line(
                [7.0, 0.0],
                [7.0, TREE_ROW_CHILD],
                HAIRLINE,
                LineCap::Round,
                guide,
            )
            .line(
                [7.0, TREE_ROW_CHILD / 2.0],
                [TREE_STEP - S4, TREE_ROW_CHILD / 2.0],
                HAIRLINE,
                LineCap::Round,
                guide,
            )
    };
    canvas(shape).size(TREE_STEP, TREE_ROW_CHILD).a11y_hidden()
}
