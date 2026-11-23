//! Activity rows, folded bursts, tool rows, and shared activity-grid helpers.

use rune::{El, Icon, col, el, icon, row, text};
use rune_kit::ProgressCircle;

use super::{message, output, subagents};
use crate::{
    app::Desktop,
    model::{
        ActivityMark, ActivityTally, AgentStanding, Item, SubagentCall, Tool, ToolAction,
        ToolStatus, delegated_task, humanize_tool, is_command, local_child, relative_path,
        spawned_subagent, split_path, subagent_call,
    },
    theme::{self, HAIRLINE, ICON, ICON_SMALL, S2, S4, S8, Size, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// An activity row's height.
const ACTIVITY_ROW: f32 = Size::Small.height();
/// Where an activity row's words start, and where what it opens starts.
pub(super) const INDENT: f32 = ICON + S8;

/// A single tool has its own disclosure; a lone delegation is a task card.
pub(super) fn single_tool(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    match subagent_call(tool) {
        Some(SubagentCall::Spawn) if spawned_subagent(tool) => {
            subagents::agent_stack(s, std::iter::once(tool), t)
        }
        Some(call) if call != SubagentCall::Spawn && tool.status != ToolStatus::Failed => {
            subagents::subagent_row(s, tool, call, t)
        }
        _ => tool_row(s, tool, t),
    }
}

/// Renders a folded or expanded burst of consecutive reasoning and tool items.
pub(super) fn activity_group<'a>(
    s: &Desktop,
    first: &str,
    burst: &(impl Iterator<Item = (&'a Item, bool)> + Clone),
    t: &Theme,
) -> El {
    let m = &s.model;
    let key = format!("{first}-activity");
    let open = m.disclosure.expanded.contains(&key);
    let tally = ActivityTally::of(m, burst.clone());
    let only_thought = tally.only_thought();
    let can_open = !only_thought || tally.last_thought.is_some();
    let label = tally.label();
    let mut heading = line("header", mark(activity_icon(tally.mark()), t))
        .label(label.clone())
        .child(
            text(label)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .shrink(),
        );
    if can_open {
        heading = disclosure(heading, &key, open, t);
    }
    let preview = if can_open {
        tally
            .last_thought
            .filter(|_| !open && (only_thought || tally.thinking))
            .and_then(|thought| thought.lines().next())
            .map(|thought| thought.trim_matches(['*', '_', '#', ' ']))
    } else {
        (!tally.thinking).then_some("Reasoning hidden")
    };
    heading = heading.child(
        text(preview.unwrap_or_default())
            .font(t.ui)
            .color(t.faint)
            .grow(1.0)
            .min_w(0.0)
            .shrink()
            .truncate(),
    );
    if tally.spawns > 0 {
        let marks = burst.clone().filter_map(|(item, _)| match item {
            Item::Tool(tool) if spawned_subagent(tool) => {
                Some(local_child(m, tool, delegated_task(m, tool)))
            }
            _ => None,
        });
        heading = heading.child(subagents::agent_marks(marks, tally.spawns, t));
    }
    if let Some(status) = AgentStanding::summary(&tally.agents) {
        heading = heading.child(
            text(status)
                .font(t.small)
                .color(t.muted)
                .shrink()
                .truncate(),
        );
    }
    if tally.failures > 0 {
        heading = heading.child(
            text(format!("{} failed", tally.failures))
                .font(t.small)
                .color(t.danger),
        );
    }
    if tally.thinking {
        heading = heading.child(ui::spinner(t));
    }
    let mut group = col().key(key).w_full().gap(S2).child(heading);
    if can_open && open {
        group = group.child(activity_details(s, burst, t));
    } else if let Some(tool) = tally.active_tool {
        group = group.child(single_tool(s, tool, t));
    }
    group
}

fn activity_details<'a>(
    s: &Desktop,
    burst: &(impl Iterator<Item = (&'a Item, bool)> + Clone),
    t: &Theme,
) -> El {
    let mut details = col().key("details").w_full().gap(S2);
    let mut entries = burst.clone().peekable();
    while let Some((item, streaming)) = entries.next() {
        match item {
            Item::Thinking { key, text } => {
                let text = text.trim();
                if !text.is_empty() {
                    details = details.child(message::thought(key, text, streaming, t));
                }
            }
            Item::Tool(tool) if spawned_subagent(tool) => {
                let others = std::iter::from_fn(|| match entries.peek() {
                    Some((Item::Tool(tool), _)) if spawned_subagent(tool) => {
                        let tool = tool.as_ref();
                        entries.next();
                        Some(tool)
                    }
                    _ => None,
                });
                details = details.child(
                    col()
                        .key(format!("{}-tasks", tool.key))
                        .w_full()
                        .pl(INDENT)
                        .py(S4)
                        .child(subagents::agent_stack(
                            s,
                            std::iter::once(tool.as_ref()).chain(others),
                            t,
                        )),
                );
            }
            Item::Tool(tool) => details = details.child(single_tool(s, tool, t)),
            _ => {}
        }
    }
    details
}

const fn action_icon(action: ToolAction) -> &'static Icon {
    match action {
        ToolAction::Command => &ui::TERMINAL,
        ToolAction::Read => &ui::FILE_READ,
        ToolAction::Write => &ui::FILE_WRITE,
        ToolAction::Edit | ToolAction::Move => &ui::FILE_EDIT,
        ToolAction::Delete => &ui::TRASH,
        ToolAction::Search => &ui::SEARCH,
        ToolAction::Fetch => &ui::GLOBE,
        ToolAction::Other => &ui::WRENCH,
    }
}

const fn activity_icon(kind: ActivityMark) -> &'static Icon {
    match kind {
        ActivityMark::Brain => &ui::BRAIN,
        ActivityMark::Agent => &ui::AGENT,
        ActivityMark::Action(action) => action_icon(action),
    }
}

/// An activity mark: a small Lucide icon in muted ink, text ink while its row
/// is hovered.
pub(super) fn mark(glyph: &'static Icon, t: &Theme) -> El {
    icon(glyph, ICON_SMALL).color(t.muted).group_color(t.text)
}

/// An activity row: its mark in the icon column, then whatever follows at
/// [`INDENT`], word-spaced.
pub(super) fn line(key: &str, mark: El) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(ACTIVITY_ROW)
        .gap(S4)
        .center_y()
        .child(ui::lead(mark, ACTIVITY_ROW).mr(S4))
}

/// A row that does something on press: focusable, ink-only hover.
pub(super) fn pressable(row: El, t: &Theme) -> El {
    row.focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .radius(Size::Small.radius())
        .hover_group()
}

/// `row` opens and closes `key` on press, with a chevron after its words.
pub(super) fn disclosure(row: El, key: &str, open: bool, t: &Theme) -> El {
    let toggle = key.to_owned();
    pressable(row, t)
        .role("button")
        .expanded(open)
        .on_press(move |s: &mut Desktop| {
            s.dispatch_local(Msg::Timeline(TimelineMsg::ToggleExpanded(toggle.clone())));
        })
        .child(chevron(open, t))
}

pub(super) fn chevron(open: bool, t: &Theme) -> El {
    icon(
        if open {
            &ui::CHEVRON_OPEN
        } else {
            &ui::CHEVRON
        },
        ICON_SMALL,
    )
    .color(t.faint)
    .group_color(t.text)
    .a11y_hidden()
}

/// A file as its name in full ink and its directory after it, faint.
fn file_target(s: &Desktop, path: &str, t: &Theme) -> El {
    let (dir, name) = split_path(relative_path(&s.model, path));
    row()
        .key("target")
        .gap(S8)
        .min_w(0.0)
        .shrink()
        .center_y()
        .help(path)
        .child(text(name).font(t.ui).color(t.text).truncate().min_w(0.0))
        .children((!dir.is_empty()).then(|| {
            text(dir.trim_end_matches('/'))
                .font(t.ui)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0)
        }))
}

/// Where a call stands, at the row's end: nothing once it's done.
pub(super) fn meta(tool: &Tool, now: i64, t: &Theme) -> Option<El> {
    let faint = |words: &str| text(words.to_owned()).font(t.small).color(t.faint);
    match tool.status {
        ToolStatus::Running if tool.timeout_ms.is_some() => tool.timeout_ms.map(|limit| {
            let label = format!("Timeout: {}", theme::duration(limit));
            tool.started_at.map_or_else(
                || {
                    icon(&ui::TIMER, ICON_SMALL)
                        .color(t.muted)
                        .help(label.clone())
                },
                |start| {
                    let elapsed = u64::try_from(now.saturating_sub(start)).unwrap_or_default();
                    ProgressCircle::new(theme::fraction(elapsed, limit))
                        .size(ICON_SMALL)
                        .label("Command time limit")
                        .build(&t.k)
                        .help(label.clone())
                },
            )
        }),
        ToolStatus::Running => Some(ui::spinner(t)),
        ToolStatus::Pending => Some(faint("Queued")),
        ToolStatus::Abandoned => Some(faint("Stopped")),
        ToolStatus::Failed => Some(
            text(
                tool.exit_code
                    .filter(|&code| code != 0)
                    .map_or_else(|| "Failed".to_owned(), |code| format!("Exit {code}")),
            )
            .font(t.small)
            .color(t.danger),
        ),
        ToolStatus::Done => None,
    }
}

fn tool_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let m = &s.model;
    let action = ToolAction::of(tool);
    let expandable = !tool.output.is_empty() || !tool.diffs.is_empty();
    let open = expandable && m.disclosure.expanded.contains(&tool.key);
    let done = !matches!(tool.status, ToolStatus::Pending | ToolStatus::Running);
    let generic = matches!(tool.name.as_str(), "tool" | "other" | "");
    let verb = action.verb(done).map_or_else(
        || (!generic).then(|| humanize_tool(&tool.name)),
        |verb| Some(verb.to_owned()),
    );
    let detail = tool.summary.as_str();
    let mut glyph = icon(action_icon(action), ICON_SMALL);
    glyph = if tool.status == ToolStatus::Failed {
        glyph.color(t.danger)
    } else {
        glyph.color(t.muted).group_color(t.text)
    };
    let mut header = line("tool", glyph).label(match &verb {
        Some(verb) if !detail.is_empty() => format!("{verb} {detail}"),
        Some(verb) => verb.clone(),
        None => detail.to_owned(),
    });
    if let Some(verb) = &verb {
        header = header.child(
            text(verb.clone())
                .font(t.ui)
                .color(t.muted)
                .group_color(t.text),
        );
    }
    let target = if detail.is_empty() {
        None
    } else if is_command(&tool.name) {
        let mut command = text(detail)
            .font(t.mono)
            .color(t.text)
            .min_w(0.0)
            .truncate()
            .shrink();
        command = match tool.input.as_deref().filter(|input| *input != detail) {
            Some(full) => command.help(full),
            None => command.help_when_truncated(detail),
        };
        Some(command)
    } else if action.on_file() {
        Some(file_target(s, detail, t))
    } else {
        Some(
            text(detail)
                .font(t.ui)
                .color(if verb.is_some() { t.text } else { t.muted })
                .min_w(0.0)
                .truncate()
                .shrink()
                .help_when_truncated(detail),
        )
    };
    header = header.children(target);
    if expandable {
        header = disclosure(header, &tool.key, open, t);
    }
    header = header
        .child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(meta(tool, m.disclosure.tool_clock, t));
    let mut stack = col().key(tool.key.clone()).w_full().child(header);
    if open {
        stack = stack.child(output::tool_output(s, tool, t));
    }
    stack
}
