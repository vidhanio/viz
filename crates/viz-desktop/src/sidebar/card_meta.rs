//! Thread row metadata and the drag insertion indicator.

use rune::{El, el, icon, row, text};
use viz_client::protocol::{GitHubPull, GitHubPullState, Project, ThreadStatus, ThreadSummary};

use crate::{
    theme::{ICON_MD, ICON_SMALL, S2, S4, S6, Theme},
    ui,
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

/// Branch, pull request and harness metadata.
pub(super) fn metadata_row(
    thread: &ThreadSummary,
    project: Option<&Project>,
    selected: bool,
    t: &Theme,
) -> El {
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
