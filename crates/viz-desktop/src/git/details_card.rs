//! Adaptive, flat thread details shared by the inline lane and header popover.

use std::collections::HashSet;

use rune::{El, col, el, text};
use rune_kit::Placement;
use viz_client::protocol::{TaskStatus, ThreadId, ThreadStatus};

use crate::{
    app::Desktop,
    effects::git::Request,
    model::{
        AgentStanding, Item, child_link, delegated_task, first_non_empty_line,
        git::{self, CARD_INSET, CARD_WIDTH, Density},
        local_child, spawned_subagent, subagent_title, thread_title,
    },
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
        subagents(s, id, t),
        usage(s, t),
        version_control(s, id, density, t, layer),
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

fn subagents(s: &Desktop, id: ThreadId, t: &Theme) -> Option<El> {
    let m = &s.model;
    let transcript = m.transcript();
    let tools = transcript
        .displayed_history()
        .iter()
        .chain(&transcript.acp_partial)
        .rev()
        .filter_map(|item| match item {
            Item::Tool(tool) if spawned_subagent(tool) => Some(tool),
            _ => None,
        });
    let is_local = |environment: Option<&str>| {
        environment.is_none_or(|env| Some(env) == m.connection.environment.as_deref())
    };
    let mut tasks = HashSet::new();
    let mut local_threads = HashSet::new();
    let mut rows = Vec::new();
    for tool in tools {
        let task = delegated_task(m, tool);
        if task.is_some_and(|task| !tasks.insert(task.id.as_str())) {
            continue;
        }
        if let Some((thread, env)) = child_link(tool, task)
            && is_local(env)
            && !local_threads.insert(thread)
        {
            continue;
        }
        let child = local_child(m, tool, task);
        let title = subagent_title(tool, task, child);
        let title = if title.is_empty() {
            "Delegated task"
        } else {
            title
        };
        let standing = AgentStanding::of(m, tool, task);
        let target = child_link(tool, task)
            .filter(|_| task.is_none_or(|task| task.reachable))
            .map(|(thread, env)| (thread, env.map(str::to_owned)));
        let item = subagent_link(
            tool.key.clone(),
            title,
            standing,
            task_preview(task),
            target,
            t,
        );
        rows.push(task_controls(s, item, task, t));
    }
    for task in m.tasks().iter().filter(|task| task.parent == id) {
        if tasks.contains(task.id.as_str()) {
            continue;
        }
        if let Some(thread) = task.thread
            && is_local(Some(&task.environment))
            && !local_threads.insert(thread)
        {
            continue;
        }
        let target = task
            .thread
            .filter(|_| task.reachable)
            .map(|thread| (thread, Some(task.environment.clone())));
        let row = subagent_link(
            format!("linked-task-{}", task.id),
            &task.title,
            AgentStanding::from_task(task),
            task_preview(Some(task)),
            target,
            t,
        );
        rows.push(task_controls(s, row, Some(task), t));
    }
    for child in m.cache().threads().filter(|child| {
        child.parent == Some(id)
            && is_local(child.parent_environment.as_deref())
            && !local_threads.contains(&child.id)
            && !child.lifecycle.archived
    }) {
        let standing = match child.status {
            ThreadStatus::Working => AgentStanding::Working,
            ThreadStatus::Failed => AgentStanding::Failed,
            ThreadStatus::Idle => AgentStanding::Done,
        };
        rows.push(subagent_link(
            format!("linked-thread-{}", child.id),
            thread_title(child),
            standing,
            None,
            Some((child.id, None)),
            t,
        ));
    }
    (!rows.is_empty()).then(|| section("Subagents", rows, t))
}

fn subagent_link(
    key: String,
    title: &str,
    standing: AgentStanding,
    subtitle: Option<&str>,
    target: Option<(ThreadId, Option<String>)>,
    t: &Theme,
) -> El {
    let ink = match standing {
        AgentStanding::Working => t.accent,
        AgentStanding::Failed => t.danger,
        AgentStanding::Done => t.success,
        AgentStanding::Queued | AgentStanding::Stopped | AgentStanding::Unreachable => t.muted,
    };
    let mut row = ui::list_row(key, title)
        .leading(ui::dot(ink))
        .trailing(text(standing.word()).font(t.micro).color(ink));
    if let Some(subtitle) = subtitle {
        row = row.subtitle(subtitle);
    }
    if let Some((thread, environment)) = target {
        row = row.on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::OpenThreadLink {
                    thread,
                    environment: environment.clone(),
                }),
                tx,
            );
        });
    }
    row.build(t)
}

fn task_preview(task: Option<&viz_client::protocol::Task>) -> Option<&str> {
    let task = task?;
    task.error
        .as_deref()
        .or(task.result.as_deref())
        .and_then(first_non_empty_line)
}

fn task_controls(
    s: &Desktop,
    item: El,
    task: Option<&viz_client::protocol::Task>,
    t: &Theme,
) -> El {
    let Some(task) = task.filter(|task| {
        task.reachable && matches!(task.status, TaskStatus::Queued | TaskStatus::Running)
    }) else {
        return item;
    };
    let id = task.id.clone();
    rune::row()
        .key(format!("subagent-task-{id}"))
        .w_full()
        .center_y()
        .child(item.grow(1.0).shrink().min_w(0.0))
        .child(
            ui::icon_button(
                &ui::STOP,
                "Cancel task",
                rune_kit::IconButtonVariant::GhostMuted,
                crate::theme::Size::Micro,
            )
            .disabled(!s.model.connected())
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::CancelTask(id.clone())), tx);
            })
            .build(&t.k),
        )
}

fn usage(s: &Desktop, t: &Theme) -> Option<El> {
    let usage = s.model.thread_state()?.usage.as_ref()?;
    if usage.size == 0 {
        return None;
    }
    let width = (u128::from(usage.used.min(usage.size)) * 244) / u128::from(usage.size.max(1));
    let width = f32::from(u16::try_from(width).unwrap_or(244));
    let bar = el()
        .w_full()
        .h(4.0)
        .radius(2.0)
        .fill(t.hairline)
        .child(el().w(width).h(4.0).radius(2.0).fill(t.accent));
    Some(section(
        "Context Window",
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
