//! Timeline subagent cards, delegated task results, and subagent interaction
//! rows.

use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, Markdown};
use viz_client::protocol::ThreadSummary;

use super::activity::{INDENT, chevron, disclosure, line, mark, meta, pressable};
use crate::{
    app::Desktop,
    header,
    model::{
        AgentStanding, SubagentCall, TaskResult, Tool, child_link, delegated_task, local_child,
        subagent_title,
    },
    theme::{CARD_INSET, ICON, ICON_SMALL, S2, S4, S8, Size, Theme},
    ui::{self, Card},
    update::{Msg, TimelineMsg},
};

/// A delegated task's outcome arriving: its title and how it ended, opening
/// its thread; the result itself opens beneath.
pub(super) fn task_result(s: &Desktop, result: &TaskResult, t: &Theme) -> El {
    let m = &s.model;
    let task = m.tasks().iter().find(|task| task.id == result.task);
    let title = task
        .map(|task| task.title.as_str())
        .filter(|title| !title.is_empty())
        .or_else(|| (!result.title.is_empty()).then_some(result.title.as_str()))
        .unwrap_or("Delegated task");
    let (word, ink) = match result.status.as_str() {
        "completed" => ("finished", t.muted),
        "continued" => ("continued", t.muted),
        "failed" => ("failed", t.danger),
        "cancelled" => ("was stopped", t.muted),
        _ => ("ended", t.muted),
    };
    let open = !result.detail.is_empty() && m.disclosure.expanded.contains(&result.key);
    let mut heading = line("heading", mark(&ui::INBOX, t))
        .label(format!("{title} {word}"))
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .shrink(),
        )
        .child(text(word).font(t.ui).color(ink));
    if !result.detail.is_empty() {
        heading = disclosure(heading, &result.key, open, t);
    }
    let preview = (!open)
        .then(|| result.detail.lines().find(|line| !line.trim().is_empty()))
        .flatten();
    heading = heading.child(
        text(preview.unwrap_or_default())
            .font(t.ui)
            .color(t.faint)
            .truncate()
            .shrink()
            .grow(1.0)
            .min_w(0.0),
    );
    if let Some((child, environment)) = task
        .filter(|task| task.reachable)
        .and_then(|task| task.thread.map(|child| (child, task.environment.clone())))
    {
        heading = heading.child(
            ui::button("Open", ButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenThreadLink {
                            thread: child,
                            environment: Some(environment.clone()),
                        }),
                        tx,
                    );
                })
                .build(&t.k)
                .key("open"),
        );
    }
    let mut block = col().key(result.key.clone()).w_full().child(heading);
    if open {
        block = block.child(
            col().key("detail").w_full().pl(INDENT).pt(S2).pb(S8).child(
                Markdown::new(result.detail.clone())
                    .style(t.markdown().body_font(t.ui).text(t.muted).link(t.text))
                    .selectable(true)
                    .on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
                        s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
                    })
                    .build(&t.k)
                    .w_full(),
            ),
        );
    }
    block
}

/// Up to three delegated agents' flat monochrome harness icons.
pub(super) fn agent_marks<'a>(
    agents: impl Iterator<Item = Option<&'a ThreadSummary>>,
    total: usize,
    t: &Theme,
) -> El {
    const MARKS: usize = 3;
    let mut stack = row().gap(S4).center_y().a11y_hidden();
    for thread in agents.take(MARKS) {
        let glyph = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
        stack = stack.child(icon(glyph, ICON_SMALL).color(t.muted));
    }
    if total > MARKS {
        stack = stack.child(
            text(format!("+{}", total - MARKS))
                .font(t.small)
                .color(t.muted),
        );
    }
    stack
}

/// Delegated tasks stacked in one outlined card separated by hairlines.
pub(super) fn agent_stack<'a>(s: &Desktop, tools: impl Iterator<Item = &'a Tool>, t: &Theme) -> El {
    let mut tools = tools.peekable();
    let key = tools.peek().map_or("tasks", |tool| tool.key.as_str());
    let mut card = Card::Outline
        .frame(t)
        .key(format!("{key}-stack"))
        .role("list")
        .label("Delegated tasks");
    for (index, tool) in tools.enumerate() {
        if index > 0 {
            card = card.child(ui::rule(t).key(format!("rule-{index}")));
        }
        card = card.child(subagent_link(s, tool, t));
    }
    card
}

/// Talking to, reading, listing or stopping a subagent: one activity row
/// that opens the subagent it names.
pub(super) fn subagent_row(s: &Desktop, tool: &Tool, call: SubagentCall, t: &Theme) -> El {
    let m = &s.model;
    let verb = match call {
        SubagentCall::Spawn => "Started",
        SubagentCall::Message => "Messaged",
        SubagentCall::Read => "Read",
        SubagentCall::List => "Listed tasks",
        SubagentCall::Cancel => "Stopped",
    };
    let task = delegated_task(m, tool);
    let thread = local_child(m, tool, task);
    let glyph = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
    let name = (tool.child.is_some() || task.is_some())
        .then(|| subagent_title(tool, task, thread))
        .filter(|title| !title.is_empty());
    let verb = match (call, name) {
        (SubagentCall::Read, None) => "Checked thread activity",
        (SubagentCall::Message, None) => "Sent a message",
        (SubagentCall::Cancel, None) => "Stopped a task",
        _ => verb,
    };
    let label = name.map_or_else(|| verb.to_owned(), |name| format!("{verb} {name}"));
    let mut row = line(&tool.key, mark(glyph, t))
        .label(label)
        .child(text(verb).font(t.ui).color(t.muted).group_color(t.text))
        .children(name.map(|name| {
            text(name)
                .key("title")
                .font(t.ui)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
        }));
    let environment = tool.child_environment.clone();
    let link = tool
        .child
        .filter(|_| task.is_none_or(|task| task.reachable));
    if let Some(child) = link {
        row = pressable(row, t)
            .role("link")
            .help("Open thread")
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: child,
                        environment: environment.clone(),
                    }),
                    tx,
                );
            })
            .child(chevron(false, t));
    }
    row.child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(meta(tool, m.disclosure.tool_clock, t).map(|meta| meta.key("meta")))
}

fn subagent_link(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let m = &s.model;
    let task = delegated_task(m, tool);
    let thread = local_child(m, tool, task);
    let title = subagent_title(tool, task, thread);
    let title = if title.is_empty() {
        "Delegated task"
    } else {
        title
    };
    let standing = AgentStanding::of(m, tool, task);
    let identity = task.map_or_else(
        || {
            thread.map_or_else(
                || {
                    row()
                        .gap(S8)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .center_y()
                        .child(icon(&ui::AGENT, ICON).color(t.muted))
                        .child(
                            text(title)
                                .font(t.ui_medium)
                                .color(t.text)
                                .truncate()
                                .min_w(0.0)
                                .shrink(),
                        )
                },
                |thread| header::agent_identity(thread, t),
            )
        },
        |task| header::task_identity(task, thread, t),
    );
    let mut link = row()
        .key(tool.key.clone())
        .label(title)
        .description(standing.word())
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .child(identity.grow(1.0).shrink().min_w(0.0))
        .child(agent_mark(standing, t))
        .child(text(standing.word()).font(t.small).color(t.muted));
    if let Some((child, environment)) =
        child_link(tool, task).filter(|_| task.is_none_or(|task| task.reachable))
    {
        let environment = environment.map(str::to_owned);
        link = pressable(link, t)
            .role("link")
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: child,
                        environment: environment.clone(),
                    }),
                    tx,
                );
            })
            .child(chevron(false, t));
    }
    link
}

fn agent_mark(standing: AgentStanding, t: &Theme) -> El {
    match standing {
        AgentStanding::Working => ui::spinner(t),
        AgentStanding::Queued => ui::dot(t.muted),
        AgentStanding::Done => ui::dot(t.success),
        AgentStanding::Failed => ui::dot(t.danger),
        AgentStanding::Stopped | AgentStanding::Unreachable => ui::dot(t.faint),
    }
}
