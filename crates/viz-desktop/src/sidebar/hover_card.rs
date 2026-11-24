//! Delayed right-anchored thread hover preview card (`450ms` rest delay, flat
//! opaque `t.surface_raised` surface with `1px t.border_overlay`, zero drop
//! shadow).

use rune::{Align, El, Icon, Side, col, icon, row, text};
use viz_client::protocol::{GitHubPull, GitHubPullState, ThreadSummary};

use crate::{
    app::Desktop,
    model::{SidebarOutline, format_wake_label, thread_title},
    theme::{ICON_MD, ICON_SMALL, S4, S6, S8, S12, Theme},
    ui,
};

/// Fixed width of the floating hover preview card (points).
const HOVER_CARD_WIDTH: f32 = 248.0;

/// Mounts the floating hover preview card into `layer` when a thread card is
/// hovered and no competing popover or drag is active.
pub(super) fn mount(s: &Desktop, outline: &SidebarOutline<'_>, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    let Some(id) = m.sidebar.hover_card else {
        return;
    };
    if m.sidebar.menu.is_some()
        || m.sidebar.snooze_popover.is_some()
        || m.sidebar.filter_open
        || m.sidebar.inline_rename.is_some()
        || m.sidebar.drag.as_ref().is_some_and(|drag| drag.active)
    {
        return;
    }
    let Some(thread) = m.summary(id) else {
        return;
    };
    let anchor_key = format!("thread-{id}");
    layer.push(ui::anchored_popover(
        ui::popover_frame(format!("hover-card-{id}"), HOVER_CARD_WIDTH, t),
        &anchor_key,
        (Side::Right, Align::Start),
        S4,
        m.panels.viewport(),
        thread_preview(s, thread, outline.now, t),
    ));
}

/// Shared title and metadata for a hovered thread row.
pub fn thread_preview(s: &Desktop, thread: &ThreadSummary, now: i64, t: &Theme) -> El {
    let project = thread
        .project
        .as_deref()
        .and_then(|path| s.model.cache().project(path));
    col()
        .w_full()
        .padding(S12)
        .gap(S8)
        .child(
            text(thread_title(thread))
                .font(t.panel_row)
                .color(t.text_strong)
                .wrap()
                .max_lines(3)
                .w_full(),
        )
        .child(hover_metadata(s, now, thread, project, t))
}

fn hover_metadata(
    s: &Desktop,
    now: i64,
    thread: &ThreadSummary,
    project: Option<&viz_client::protocol::Project>,
    t: &Theme,
) -> El {
    let m = &s.model;
    let env_name = thread
        .parent_environment
        .as_deref()
        .unwrap_or_else(|| m.active_env_key());
    let is_remote = thread
        .parent_environment
        .as_deref()
        .is_some_and(|env| env != m.active_env_key())
        || m.connection
            .environments
            .iter()
            .find(|env| env.id == env_name)
            .is_some_and(|env| env.status != viz_client::protocol::EnvironmentStatus::Local);
    let branch = thread
        .worktree
        .as_ref()
        .and_then(|wt| wt.branch.as_deref())
        .or_else(|| project.and_then(|p| p.branch.as_deref()));

    let mut meta = col().w_full().gap(5.0);
    meta = meta.child(
        row()
            .w_full()
            .gap(S6)
            .center_y()
            .child(ui::project_mark(s, project, ICON_MD, t))
            .child(
                text(project.map_or("No project", |p| p.name.as_str()))
                    .font(t.small)
                    .color(t.muted)
                    .truncate()
                    .shrink()
                    .min_w(0.0),
            ),
    );
    meta = meta.child(meta_row(
        if is_remote { &ui::SERVER } else { &ui::MONITOR },
        if is_remote { t.remote } else { t.faint },
        env_name,
        t,
    ));
    if let Some(branch) = branch {
        let label = if thread.worktree.is_some() {
            format!("{branch} · worktree")
        } else {
            branch.to_owned()
        };
        meta = meta.child(meta_row(&ui::GIT_BRANCH, t.faint, &label, t));
    }
    let model_label = model_summary(thread);
    meta = meta.child(meta_row(
        ui::harness_icon(&thread.harness),
        t.muted,
        &model_label,
        t,
    ));
    if let Some(pr) = &thread.pull_request {
        meta = meta.child(pr_meta_row(pr, t));
    }
    if thread.waiting_on > 0 {
        let label = if thread.waiting_on == 1 {
            "1 delegated task running".to_owned()
        } else {
            format!("{} delegated tasks running", thread.waiting_on)
        };
        meta = meta.child(meta_row(&ui::AGENT, t.delegated, &label, t));
    }
    if let Some(until) = thread
        .lifecycle
        .snoozed_until
        .filter(|&until| now == 0 || until > now)
    {
        let label = format!("Snoozed · wakes {}", format_wake_label(until, now));
        meta = meta.child(meta_row(&ui::CLOCK, t.faint, &label, t));
    }
    meta
}

fn meta_row(glyph: &'static Icon, ink: rune::Color, label: &str, t: &Theme) -> El {
    row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(icon(glyph, 13.0).color(ink).a11y_hidden())
        .child(
            text(label)
                .font(t.small)
                .color(t.muted)
                .truncate()
                .shrink()
                .min_w(0.0),
        )
}

fn pr_meta_row(pr: &GitHubPull, t: &Theme) -> El {
    let (glyph, color) = match pr.state {
        GitHubPullState::Open if pr.draft => (&ui::PR_DRAFT, t.faint),
        GitHubPullState::Open => (&ui::PR_OPEN, t.success),
        GitHubPullState::Merged => (&ui::GIT_MERGE, t.delegated),
        GitHubPullState::Closed => (&ui::PR_CLOSED, t.danger),
    };
    let mut item = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(icon(glyph, ICON_SMALL).color(color).a11y_hidden())
        .child(
            text(format!("#{} {}", pr.number, pr.title))
                .font(t.small)
                .color(t.muted)
                .truncate()
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        );
    if pr.additions > 0 || pr.deletions > 0 {
        item = item.child(
            row()
                .gap(S4)
                .center_y()
                .child(
                    text(format!("+{}", pr.additions))
                        .font(t.mono_micro)
                        .color(t.success),
                )
                .child(
                    text(format!("-{}", pr.deletions))
                        .font(t.mono_micro)
                        .color(t.danger),
                ),
        );
    }
    item
}

fn model_summary(thread: &ThreadSummary) -> String {
    let thinking = thread.thinking.as_deref().map(crate::model::thinking_label);
    match (thread.model.as_deref(), thinking) {
        (Some(model), Some(thinking)) => format!("{model} · {thinking}"),
        (Some(model), None) => model.to_owned(),
        (None, Some(thinking)) => format!("{} · {thinking}", thread.harness_name),
        (None, None) => thread.harness_name.clone(),
    }
}
