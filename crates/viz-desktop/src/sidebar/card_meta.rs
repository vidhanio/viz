//! Thread card Row 3 metadata (branch/worktree, PR status badge, harness glyph,
//! delegated child indicator), delegated child subtree rows, and drag drop bar.

use rune::{Color, El, LineCap, Shape, canvas, el, icon, row, text};
use viz_client::protocol::{
    GitHubPull, GitHubPullState, Project, TaskStatus, ThreadStatus, ThreadSummary,
};

use crate::{
    app::Desktop,
    model::{SidebarOutline, thread_title},
    theme::{
        HAIRLINE, ICON, ICON_MD, ICON_SMALL, RADIUS_CONTROL, RADIUS_XS, S2, S4, S6, TREE_ROW_CHILD,
        TREE_STEP, Theme,
    },
    ui,
    update::{Msg, SelectionMsg, SidebarMsg},
};

/// Crisp `2px` `accent` horizontal drop insertion bar with a `4px` left end
/// cap (`sidebar.md` §4 & §7).
pub(super) fn drop_indicator(key: impl Into<String>, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .h(6.0)
        .px(S4)
        .center_y()
        .a11y_hidden()
        .child(el().size(4.0, 4.0).radius(2.0).fill(t.accent))
        .child(el().grow(1.0).h(2.0).radius(1.0).fill(t.accent))
}

/// Row 3: branch/worktree label, GitHub PR status badge, delegated-children
/// count/toggle, failure alert, and harness glyph.
pub(super) fn metadata_row(
    thread: &ThreadSummary,
    project: Option<&Project>,
    kid_count: usize,
    show_kids: bool,
    selected: bool,
    t: &Theme,
) -> El {
    let id = thread.id;
    let worktree_branch = thread
        .worktree
        .as_ref()
        .map(|wt| wt.branch.as_deref().unwrap_or("detached"));
    let branch = worktree_branch.or_else(|| project.and_then(|p| p.branch.as_deref()));

    let mut left = row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S6)
        .center_y()
        .a11y_hidden();
    if let Some(branch) = branch {
        if thread.worktree.is_some() {
            left = left.child(icon(&ui::GIT_BRANCH, 11.0).color(t.faint));
        }
        left = left.child(
            text(branch)
                .font(t.small)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0),
        );
    }
    if let Some(pr) = &thread.pull_request {
        left = left.child(pr_badge(pr, t));
    }
    if kid_count > 0 {
        let toggle = row()
            .key(format!("child-toggle-{id}"))
            .role("button")
            .label(if show_kids {
                "Collapse delegated tasks"
            } else {
                "Expand delegated tasks"
            })
            .h(16.0)
            .px(S4)
            .gap(S2)
            .center_y()
            .radius(RADIUS_XS)
            .fill(t.delegated.alpha(0.12))
            .on_press(move |s: &mut Desktop| {
                s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleParent(id)));
            })
            .child(icon(&ui::AGENT, 10.0).color(t.delegated).a11y_hidden())
            .child(
                text(kid_count.to_string())
                    .font(t.mono_micro)
                    .color(t.delegated)
                    .a11y_hidden(),
            );
        left = left.child(toggle);
    } else if thread.waiting_on > 0 {
        left = left.child(
            row()
                .gap(S2)
                .center_y()
                .child(icon(&ui::AGENT, 10.0).color(t.delegated))
                .child(
                    text(thread.waiting_on.to_string())
                        .font(t.mono_micro)
                        .color(t.delegated),
                ),
        );
    }

    let mut right = row().gap(S4).center_y();
    if thread.status == ThreadStatus::Failed {
        right = right.child(
            icon(&ui::CIRCLE_ALERT, ICON_SMALL)
                .color(t.danger)
                .help("Last run failed"),
        );
    }
    right = right.child(
        icon(ui::harness_icon(&thread.harness), ICON_MD)
            .color(if selected { t.text } else { t.icon_muted })
            .group_color(t.text)
            .help(&thread.harness_name),
    );

    row()
        .key("row3")
        .w_full()
        .h(16.0)
        .gap(S6)
        .center_y()
        .child(left)
        .child(right)
}

fn pr_badge(pr: &GitHubPull, t: &Theme) -> El {
    let (glyph, color) = match pr.state {
        GitHubPullState::Open if pr.draft => (&ui::PR_DRAFT, t.faint),
        GitHubPullState::Open => (&ui::PR_OPEN, t.success),
        GitHubPullState::Merged => (&ui::GIT_MERGE, t.delegated),
        GitHubPullState::Closed => (&ui::PR_CLOSED, t.danger),
    };
    row()
        .gap(S2)
        .center_y()
        .child(icon(glyph, 11.0).color(color))
        .child(
            text(format!("#{}", pr.number))
                .font(t.mono_micro)
                .color(color),
        )
}

/// Renders compact indented child rows beneath an expanded parent thread card.
pub(super) fn child_rows<'a>(
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
            let show_grand = grand_active || !s.model.sidebar.collapsed_parents.contains(&child.id);
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
    let indent = f32::from(depth.saturating_sub(1)).mul_add(TREE_STEP, 14.0);
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
        .hover(if selected { t.selected } else { t.hover_wash })
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

fn child_guide(last: bool, highlighted: bool, t: &Theme) -> El {
    let guide = if highlighted {
        t.delegated.alpha(0.55)
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
