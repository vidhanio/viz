//! Subagent rows (`24×24` circular avatar + bottom-right status dot),
//! overlapping-avatar `SubagentGroup` cards, subagent interaction rows, and
//! delegated `TaskResult` delivery cards.

use std::{borrow::Cow, f32::consts::FRAC_PI_2};

use rune::{Color, El, Icon, Tx, col, el, icon, row, text};
use viz_client::protocol::{Task, ThreadSummary};

use super::work_log;
use crate::{
    app::Desktop,
    model::{
        AgentStanding, SubagentCall, Tool, TurnView, child_link, delegated_task,
        first_non_empty_line, local_child, subagent_title,
    },
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_CARD, RADIUS_PILL, RADIUS_SM, S2, S4, S6,
        S8, S24, Theme,
    },
    ui::{self, BadgeSize, Tone},
    update::{Msg, TimelineMsg},
};

/// Maximum overlapping circular avatars drawn in a `SubagentGroup` header.
const MAX_AVATARS: usize = 3;
/// Diameter of the single-subagent circular avatar (`24pt`).
const AVATAR_SIZE: f32 = 24.0;
/// Diameter of each overlapping group avatar (`22pt`).
const GROUP_AVATAR_SIZE: f32 = 22.0;
/// Diameter of the bottom-right status dot on a subagent avatar (`8pt`).
const STATUS_DOT_SIZE: f32 = 8.0;

/// Renders either a single subagent row (`== 1`) or an expandable
/// overlapping-avatar `SubagentGroup` (`>= 2`) for `turn`.
pub fn subagents_section(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> Option<El> {
    match turn.subagents.as_slice() {
        [] => None,
        [only] => Some(
            col()
                .key(format!("{}-subagent-card", turn.key))
                .w_full()
                .my(S4)
                .radius(RADIUS_CARD)
                .fill(t.surface)
                .border(HAIRLINE, t.hairline)
                .clip_rounded(RADIUS_CARD)
                .child(subagent_card_row(s, only, t)),
        ),
        many => Some(subagent_group(s, turn, many, t)),
    }
}

fn subagent_group(s: &Desktop, turn: &TurnView<'_>, tools: &[&Tool], t: &Theme) -> El {
    let m = &s.model;
    let fold_key = turn.subagents_key();
    let legacy_key = turn.legacy_fold_key.as_deref();
    let open = m.timeline.is_open(
        m.selection.selected,
        &m.disclosure.expanded,
        &fold_key,
        legacy_key,
        false,
    );

    let (counts, avatars) = group_summary(m, tools);
    let stack = avatar_stack(avatars, tools.len(), t);
    let title = format!("{} subagents", tools.len());
    let summary = AgentStanding::summary(&counts);
    let any_running = counts[AgentStanding::Working.index()] > 0;
    let header_key = format!("{fold_key}-head");
    let toggle_key = fold_key.clone();
    let toggle_legacy = turn.legacy_fold_key.clone();
    let anchor = header_key.clone();

    let header = row()
        .key(header_key)
        .role("button")
        .label(title.clone())
        .expanded(open)
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::ToggleFold {
                    key: toggle_key.clone(),
                    legacy_key: toggle_legacy.clone(),
                    default_open: false,
                    anchor: Some(anchor.clone()),
                }),
                tx,
            );
        })
        .child(stack)
        .child(text(title).font(t.ui_medium).color(t.text))
        .child(summary.map_or_else(
            || el().grow(1.0).min_w(0.0).a11y_hidden(),
            |summary| {
                text(summary)
                    .font(t.small)
                    .color(t.muted)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate()
            },
        ))
        .children(any_running.then(|| ui::spinner(t)))
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .key("chevron")
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        );

    let mut card = motion::fold_container(
        col()
            .key(fold_key)
            .w_full()
            .my(S4)
            .radius(RADIUS_CARD)
            .fill(t.surface)
            .border(HAIRLINE, t.hairline)
            .clip_rounded(RADIUS_CARD),
    )
    .child(header);

    if open {
        let mut list = col()
            .key("rows")
            .role("list")
            .label("Delegated subagents")
            .w_full();
        for tool in tools {
            list = list.child(ui::rule(t)).child(subagent_card_row(s, tool, t));
        }
        card = card.child(motion::fold_row(list));
    }

    card
}

fn group_summary(
    m: &crate::model::Model,
    tools: &[&Tool],
) -> ([usize; AgentStanding::COUNT], Vec<&'static Icon>) {
    let mut counts = [0_usize; AgentStanding::COUNT];
    let mut avatars = Vec::new();
    for tool in tools {
        let task = delegated_task(m, tool);
        let child = local_child(m, tool, task);
        counts[AgentStanding::of(m, tool, task).index()] += 1;
        if avatars.len() < MAX_AVATARS {
            avatars.push(child.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness)));
        }
    }
    (counts, avatars)
}

fn avatar_stack(avatars: Vec<&'static Icon>, total: usize, t: &Theme) -> El {
    let mut stack = row().center_y().a11y_hidden();
    for (i, glyph) in avatars.into_iter().enumerate() {
        let mut badge = row()
            .size(GROUP_AVATAR_SIZE, GROUP_AVATAR_SIZE)
            .center()
            .radius(RADIUS_PILL)
            .fill(t.surface_recessed)
            .border(S2, t.canvas)
            .child(icon(glyph, ICON_SMALL).color(t.muted));
        if i > 0 {
            badge = badge.ml(-S6);
        }
        stack = stack.child(badge);
    }
    if total > MAX_AVATARS {
        stack = stack.child(
            row()
                .h(GROUP_AVATAR_SIZE)
                .min_w(GROUP_AVATAR_SIZE)
                .px(S4)
                .ml(-S6)
                .center()
                .radius(RADIUS_PILL)
                .fill(t.chip_fill)
                .border(S2, t.canvas)
                .child(
                    text(format!("+{}", total - MAX_AVATARS))
                        .font(t.mono_micro)
                        .color(t.muted),
                ),
        );
    }
    stack
}

fn subagent_card_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let m = &s.model;
    let task = delegated_task(m, tool);
    let child = local_child(m, tool, task);
    let standing = AgentStanding::of(m, tool, task);
    let raw_title = subagent_title(tool, task, child);
    let title = if raw_title.is_empty() {
        "Delegated task"
    } else {
        raw_title
    };
    let glyph = child.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
    let copy = subagent_copy(s, tool, task, child, title, standing, t);

    let mut line = row()
        .key(tool.key.clone())
        .label(title)
        .description(standing.word())
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .child(avatar_with_status(glyph, standing, t))
        .child(copy)
        .child(standing_trailing(standing, t));

    if let Some((target_thread, environment)) =
        child_link(tool, task).filter(|_| task.is_none_or(|task| task.reachable))
    {
        let environment = environment.map(str::to_owned);
        line = line
            .role("link")
            .help("Open subagent thread")
            .hover(t.hover_wash)
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: target_thread,
                        environment: environment.clone(),
                    }),
                    tx,
                );
            })
            .child(
                icon(&ui::CHEVRON, ICON_SMALL)
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            );
    }

    line
}

fn subagent_copy(
    s: &Desktop,
    tool: &Tool,
    task: Option<&Task>,
    child: Option<&ThreadSummary>,
    title: &str,
    standing: AgentStanding,
    t: &Theme,
) -> El {
    let subtitle = subagent_subtitle(task, child);
    let preview = task
        .and_then(|task| {
            task.error
                .as_deref()
                .and_then(first_non_empty_line)
                .or_else(|| task.result.as_deref().and_then(first_non_empty_line))
        })
        .or_else(|| first_non_empty_line(&tool.output));
    let remote_env = task
        .map(|task| task.environment.as_str())
        .or(tool.child_environment.as_deref())
        .filter(|env| s.model.connection.environment.as_deref() != Some(*env));

    let mut title_line = row().gap(S6).center_y().min_w(0.0).child(
        text(title)
            .font(t.ui_medium)
            .color(t.text)
            .truncate()
            .shrink()
            .min_w(0.0),
    );
    if let Some(env) = remote_env {
        title_line = title_line.child(
            ui::badge(env)
                .tone(Tone::Info)
                .size(BadgeSize::Nano)
                .icon(&ui::SERVER)
                .build(t)
                .key("env"),
        );
    }
    if let Some(sub) = subtitle {
        title_line = title_line.child(
            text(sub)
                .font(t.micro)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0),
        );
    }

    let mut copy = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S2)
        .child(title_line);
    if let Some(preview) = preview {
        copy = copy.child(
            text(preview)
                .font(t.small)
                .color(if standing == AgentStanding::Failed {
                    t.danger
                } else {
                    t.muted
                })
                .truncate()
                .min_w(0.0),
        );
    }
    copy
}

fn avatar_with_status(glyph: &'static Icon, standing: AgentStanding, t: &Theme) -> El {
    let dot_color = standing_color(standing, t);
    let badge = row()
        .size(AVATAR_SIZE, AVATAR_SIZE)
        .center()
        .radius(RADIUS_PILL)
        .fill(t.surface_recessed)
        .border(HAIRLINE, t.hairline)
        .child(icon(glyph, ICON_MD).color(t.muted));
    let dot = el()
        .size(STATUS_DOT_SIZE, STATUS_DOT_SIZE)
        .radius(RADIUS_PILL)
        .fill(dot_color)
        .border(S2, t.surface)
        .pin(None, None, Some(-1.0), Some(-1.0));
    el().size(AVATAR_SIZE, AVATAR_SIZE)
        .a11y_hidden()
        .child(badge)
        .child(dot)
}

const fn standing_color(standing: AgentStanding, t: &Theme) -> Color {
    match standing {
        AgentStanding::Working => t.accent,
        AgentStanding::Done => t.success,
        AgentStanding::Failed => t.danger,
        AgentStanding::Queued | AgentStanding::Stopped | AgentStanding::Unreachable => t.muted,
    }
}

fn standing_trailing(standing: AgentStanding, t: &Theme) -> El {
    let color = if standing == AgentStanding::Failed {
        t.danger
    } else {
        t.muted
    };
    let mut r = row().gap(S4).center_y();
    if standing == AgentStanding::Working {
        r = r.child(ui::spinner(t));
    }
    r.child(text(standing.word()).font(t.micro).color(color))
}

fn subagent_subtitle<'a>(
    task: Option<&'a Task>,
    child: Option<&'a ThreadSummary>,
) -> Option<Cow<'a, str>> {
    if let Some(thread) = child {
        let thinking = thread.thinking.as_deref().map(ui::thinking_label);
        return Some(match (thread.model.as_deref(), thinking) {
            (Some(model), Some(thinking)) => Cow::Owned(format!("{model} · {thinking}")),
            (Some(model), None) => Cow::Borrowed(model),
            (None, Some(thinking)) => thinking,
            (None, None) => Cow::Borrowed(thread.harness_name.as_str()),
        });
    }
    task.map(|task| Cow::Borrowed(task.environment.as_str()))
}

/// Compact `24pt` work-log row for non-spawn subagent interactions (message,
/// status read, list, cancel).
pub fn subagent_interaction_row(s: &Desktop, tool: &Tool, call: SubagentCall, t: &Theme) -> El {
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
    let glyph = thread.map_or(&ui::BOT, |thread| ui::harness_icon(&thread.harness));
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
    let mut line = row()
        .key(tool.key.clone())
        .label(label)
        .w_full()
        .min_h(S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .child(work_log::icon_slot(
            icon(glyph, ICON_MD).color(t.faint).group_color(t.text),
        ))
        .child(
            text(verb)
                .font(t.ui)
                .color(t.text_secondary)
                .group_color(t.text),
        )
        .children(name.map(|name| {
            text(name)
                .font(t.ui)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
        }));

    let environment = tool.child_environment.clone();
    if let Some(child) = tool
        .child
        .filter(|_| task.is_none_or(|task| task.reachable))
    {
        line = line
            .role("link")
            .help("Open thread")
            .hover(t.hover_wash)
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenThreadLink {
                        thread: child,
                        environment: environment.clone(),
                    }),
                    tx,
                );
            })
            .child(
                icon(&ui::CHEVRON, ICON_SMALL)
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            );
    }

    line.child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(work_log::tool_status_meta(tool, m.disclosure.tool_clock, t))
}
