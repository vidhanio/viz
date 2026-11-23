//! Adaptive, flat thread details shared by the inline lane and header popover.

use rune::{El, col, el, text};
use rune_kit::Placement;
use viz_client::protocol::TaskStatus;

use crate::{
    app::Desktop,
    effects::git::Request,
    model::git::{self, CARD_INSET, CARD_WIDTH, Density},
    theme::{HAIRLINE, RADIUS_CARD, Theme},
    ui,
    update::{GitMsg, Msg, TimelineMsg},
};

pub const ANCHOR: &str = "thread-details-trigger";

pub fn mount(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let id = s.model.selection.selected?;
    let inline = git::inline(&s.model);
    if !inline && !s.model.panels.details_open {
        return None;
    }
    let height = (s.model.panels.window_height - 100.0).max(100.0);
    let density = git::density(height);
    let overlay_start = layer.len();
    let sections = [
        workspace(s, density, t),
        version_control(s, id, density, t, layer),
        (density == Density::Full)
            .then(|| linked(s, id, t))
            .flatten(),
        (density == Density::Full).then(|| usage(s, t)).flatten(),
    ];
    let mut body = col().w_full();
    for (i, item) in sections.into_iter().flatten().enumerate() {
        if i > 0 {
            body = body.child(ui::rule(t).key(format!("details-rule-{i}")));
        }
        body = body.child(item);
    }
    let body = rune_kit::popover::fit_scroll(body)
        .key("thread-details-scroll")
        .scroll_anchor()
        .max_h(height);
    if inline {
        Some(
            col()
                .key("thread-details-card")
                .w(CARD_WIDTH)
                .mt(CARD_INSET)
                .mr(CARD_INSET)
                .radius(RADIUS_CARD)
                .fill(t.surface_raised)
                .border(HAIRLINE, t.border_overlay)
                .clip_rounded(RADIUS_CARD)
                .child(body),
        )
    } else {
        layer.insert(
            overlay_start,
            ui::anchored_popover(
                "thread-details",
                ANCHOR,
                Placement::BottomEnd,
                CARD_WIDTH,
                body,
                t,
            ),
        );
        None
    }
}

fn workspace(s: &Desktop, density: Density, t: &Theme) -> Option<El> {
    if density == Density::Essential {
        return None;
    }
    let state = s.model.thread_state()?;
    let path = state.cwd.clone();
    let environment = s.model.connection.environment.as_deref().unwrap_or("local");
    let (title, detail) = if density == Density::Full {
        (environment, Some(path.as_str()))
    } else {
        (path.as_str(), None)
    };
    let mut workspace = ui::list_row("workspace-path", title)
        .leading_icon(&ui::FOLDER)
        .trailing(ui::lead(rune::icon(&ui::COPY, 14.0).color(t.muted), 18.0));
    if let Some(detail) = detail {
        workspace = workspace.subtitle(detail);
    }
    Some(section(
        "Workspace",
        [workspace
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::CopyText(path.clone())), tx);
            })
            .build(t)],
        t,
    ))
}

fn version_control(
    s: &Desktop,
    id: viz_client::protocol::ThreadId,
    density: Density,
    t: &Theme,
    layer: &mut Vec<El>,
) -> Option<El> {
    let project = s.model.project()?;
    let mut rows = Vec::new();
    if !project.git {
        let path = project.path.clone();
        rows.push(
            ui::list_row("initialize-git", "Initialize Git")
                .leading_icon(&ui::GIT_BRANCH_PLUS)
                .disabled(!s.model.connected() || s.model.busy() || s.model.git.busy)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    super::dispatch(s, GitMsg::Request(Request::Init(path.clone())), tx);
                })
                .build(t),
        );
    } else if let Some(status) = &s.model.git.status {
        if density != Density::Essential {
            rows.extend(super::toolbar::thread(s, t, layer));
        }
        rows.push(super::branch_picker::view(s, t, layer));
        if let Some(pr) = s
            .model
            .summary(id)
            .and_then(|summary| summary.pull_request.as_ref())
            .filter(|pr| status.branch.as_deref() == Some(&pr.head))
        {
            rows.push(super::pr_row::view(s, pr, t, layer));
        }
        if density == Density::Full
            || (density == Density::Compact && (git::dirty(status) || s.model.git.busy))
        {
            rows.push(super::actions::view(s, status, t, layer));
        }
        if git::dirty(status) || !s.model.transcript().items.is_empty() {
            rows.push(
                ui::list_row("git-changes", "Changes")
                    .leading_icon(&ui::FILE_DIFF)
                    .trailing(super::diff_panel::stat(status.stats, t))
                    .on_press_tx(|s: &mut Desktop, tx| {
                        super::dispatch(s, GitMsg::ToggleDiff, tx);
                    })
                    .build(t),
            );
        }
    } else {
        rows.push(
            text("Waiting for Git status…")
                .font(t.small)
                .color(t.muted)
                .padding(8.0),
        );
    }
    Some(section("Version Control", rows, t))
}

fn linked(s: &Desktop, id: viz_client::protocol::ThreadId, t: &Theme) -> Option<El> {
    let children =
        s.model
            .tasks()
            .iter()
            .filter(|task| task.parent == id)
            .take(4)
            .map(|task| {
                let mut item = ui::list_row(format!("linked-task-{}", task.id), &task.title)
                    .leading(ui::dot(match task.status {
                        TaskStatus::Running => t.accent,
                        TaskStatus::Completed => t.success,
                        TaskStatus::Failed => t.danger,
                        TaskStatus::Queued | TaskStatus::Cancelled => t.muted,
                    }));
                if let Some(thread) = task.thread {
                    let environment = task.environment.clone();
                    item = item.on_press_tx(move |s: &mut Desktop, tx| {
                        s.dispatch(
                            Msg::Timeline(TimelineMsg::OpenThreadLink {
                                thread,
                                environment: Some(environment.clone()),
                            }),
                            tx,
                        );
                    });
                }
                item.build(t)
            })
            .collect::<Vec<_>>();
    (!children.is_empty()).then(|| section("Delegated threads", children, t))
}

fn usage(s: &Desktop, t: &Theme) -> Option<El> {
    let usage = s.model.thread_state()?.usage.as_ref()?;
    let width = (u128::from(usage.used.min(usage.size)) * 244) / u128::from(usage.size.max(1));
    let width = f32::from(u16::try_from(width).unwrap_or(244));
    let bar = el()
        .w_full()
        .h(4.0)
        .radius(2.0)
        .fill(t.hairline)
        .child(el().w(width).h(4.0).radius(2.0).fill(t.accent));
    Some(section(
        "Context Usage",
        [col()
            .px(8.0)
            .gap(6.0)
            .child(
                text(format!("{} / {} tokens", usage.used, usage.size))
                    .font(t.small)
                    .color(t.muted),
            )
            .child(bar)],
        t,
    ))
}

fn section(label: &str, rows: impl IntoIterator<Item = El>, t: &Theme) -> El {
    col()
        .key(label)
        .w_full()
        .padding(8.0)
        .child(ui::section_header(format!("details-{label}"), label).build(t))
        .children(rows)
}
