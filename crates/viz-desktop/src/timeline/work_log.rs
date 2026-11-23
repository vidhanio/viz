//! Grouped `WorkLog` summary header and compact `24pt` `WorkLogRow` entries
//! with Lucide icons, bounded scroll, and expandable
//! inspectors.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Icon, ScrollFade, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{ProgressCircle, ScrollBar, ScrollBarStyle};

use super::{inspector, subagents};
use crate::{
    app::Desktop,
    model::{
        Item, SubagentCall, Tool, ToolAction, ToolStatus, TurnView, diff_line_stats,
        first_non_empty_line, format_clock_hm, humanize_tool, is_command, relative_path,
        subagent_call, work_group_summary,
    },
    motion,
    theme::{self, HAIRLINE, ICON, ICON_SMALL, RADIUS_SM, S2, S4, S6, S16, S24, Theme},
    ui,
    update::{Msg, TimelineMsg},
};

/// Maximum height of a large collapsed-inspector work group before scrolling
/// (`max-h: 288pt` / `18rem`).
const GROUP_MAX_HEIGHT: f32 = 288.0;
/// Expanded scroll ceiling when one or more child inspectors are open inside a
/// large work group.
const GROUP_EXPANDED_MAX_HEIGHT: f32 = 620.0;
/// Entry count threshold before applying the bounded scroll container.
const BOUNDED_ENTRY_THRESHOLD: usize = 10;

/// Renders the work group summary header followed by all work-log rows of
/// `turn` (in a bounded scroll viewport when the group is large).
pub fn work_group(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> El {
    let summary = work_group_summary(&turn.work);
    let clock = turn.ended_at.and_then(format_clock_hm);
    let mut header = row()
        .key(format!("{}-group-summary", turn.key))
        .w_full()
        .min_h(S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .child(icon_slot(icon(&ui::WRENCH, ICON).color(t.faint)))
        .child(
            text(summary)
                .font(t.ui)
                .color(t.text_secondary)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    if let Some(clock) = clock {
        header = header.child(text(clock).font(t.micro).color(t.faint));
    }

    let any_child_open = turn.work.iter().any(|(item, _)| {
        s.model.timeline.is_open(
            s.model.selection.selected,
            &s.model.disclosure.expanded,
            item.key(),
            None,
            false,
        )
    });

    let rows_list = motion::fold_container(
        col()
            .key(format!("{}-group-rows", turn.key))
            .w_full()
            .gap(S2)
            .children(
                turn.work
                    .iter()
                    .map(|(item, streaming)| entry_row(s, item, *streaming, t)),
            ),
    );

    let body = if turn.work.len() > BOUNDED_ENTRY_THRESHOLD {
        let cap = if any_child_open {
            GROUP_EXPANDED_MAX_HEIGHT
        } else {
            GROUP_MAX_HEIGHT
        };
        col()
            .key(format!("{}-group-scroll", turn.key))
            .w_full()
            .max_h(cap)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_fade(ScrollFade::strength(S16).linear())
            .scroll_anchor()
            .child(rows_list)
            .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
    } else {
        rows_list
    };

    col()
        .key(format!("{}-workgroup", turn.key))
        .w_full()
        .gap(S2)
        .child(header)
        .child(body)
}

/// Renders one `WorkLogRow` (reasoning trace or tool call) plus its collapsible
/// inspector panel when expanded.
pub fn entry_row(s: &Desktop, item: &Item, streaming: bool, t: &Theme) -> El {
    match item {
        Item::Thinking {
            key,
            text: body,
            ended_at,
            ..
        } => thinking_row(s, key, body, *ended_at, streaming, t),
        Item::Tool(tool) => match subagent_call(tool) {
            Some(call) if call != SubagentCall::Spawn && tool.status != ToolStatus::Failed => {
                subagents::subagent_interaction_row(s, tool, call, t)
            }
            _ => tool_row(s, tool, t),
        },
        _ => el().key(item.key()),
    }
}

fn thinking_row(
    s: &Desktop,
    key: &str,
    body: &str,
    ended_at: Option<i64>,
    streaming: bool,
    t: &Theme,
) -> El {
    let trimmed = body.trim();
    let expandable = !trimmed.is_empty();
    let open = expandable
        && s.model.timeline.is_open(
            s.model.selection.selected,
            &s.model.disclosure.expanded,
            key,
            None,
            false,
        );
    let head = thinking_header(key, trimmed, ended_at, streaming, expandable, open, t);

    let mut stack = motion::fold_container(col().key(key.to_owned()).w_full()).child(head);
    if open {
        stack = stack.child(motion::fold_row(inspector::thought_inspector(
            key, trimmed, streaming, t,
        )));
    } else if streaming && expandable {
        stack = stack.child(
            col()
                .key(format!("{key}-live-preview"))
                .w_full()
                .pl(crate::theme::S28)
                .pb(S4)
                .child(inspector::thought_preview(trimmed, t)),
        );
    }
    stack
}

fn thinking_header(
    key: &str,
    trimmed: &str,
    ended_at: Option<i64>,
    streaming: bool,
    expandable: bool,
    open: bool,
    t: &Theme,
) -> El {
    let preview = first_non_empty_line(trimmed)
        .map(|line| {
            line.replace("**", "")
                .replace("__", "")
                .replace('`', "")
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .trim()
                .to_owned()
        })
        .filter(|line| !line.is_empty());
    let label_text = if streaming && !open && expandable {
        "Thinking".to_owned()
    } else if streaming {
        preview.unwrap_or_else(|| "Thinking…".to_owned())
    } else {
        preview.unwrap_or_else(|| "Reasoning hidden".to_owned())
    };
    let header_key = format!("{key}-row");
    let lead = row()
        .gap(S6)
        .center_y()
        .shrink()
        .min_w(0.0)
        .child(icon_slot(
            icon(&ui::BRAIN, ICON)
                .color(if streaming { t.muted } else { t.faint })
                .group_color(t.text),
        ))
        .child(
            text(label_text.clone())
                .font(t.ui)
                .color(if expandable {
                    t.text_secondary
                } else {
                    t.faint
                })
                .group_color(t.text)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    let lead = motion::text_shimmer(
        format!("{key}-shimmer"),
        streaming,
        lead,
        rune::Color::WHITE,
        if t.k.dark { 0.9 } else { 0.62 },
    );

    let mut head = row()
        .key(header_key.clone())
        .w_full()
        .min_h(S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .child(lead)
        .child(rune::spacer());

    if let Some(clock) = ended_at.and_then(format_clock_hm) {
        head = head.child(
            text(clock)
                .font(t.micro)
                .color(t.faint)
                .group_opacity(0.0, 1.0),
        );
    }
    if expandable {
        head = make_foldable(head, key.to_owned(), header_key, label_text, open, t);
    }
    head
}

fn tool_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let m = &s.model;
    let action = ToolAction::of(tool);
    let expandable = inspector::has_inspector(tool);
    let open = expandable
        && m.timeline.is_open(
            m.selection.selected,
            &m.disclosure.expanded,
            &tool.key,
            None,
            false,
        );
    let head = tool_header(s, tool, action, expandable, open, t);

    let mut stack = motion::fold_container(col().key(tool.key.clone()).w_full()).child(head);
    if open {
        stack = stack.child(motion::fold_row(inspector::tool_inspector(s, tool, t)));
    }
    stack
}

fn tool_header(
    s: &Desktop,
    tool: &Tool,
    action: ToolAction,
    expandable: bool,
    open: bool,
    t: &Theme,
) -> El {
    let running = matches!(tool.status, ToolStatus::Running);
    let (label, tooltip) = tool_display_label(s, tool, action);
    let diff_totals = (!tool.diffs.is_empty()).then(|| {
        tool.diffs.iter().fold((0_u32, 0_u32), |(a, d), diff| {
            let (add, del) = diff_line_stats(diff);
            (a.saturating_add(add), d.saturating_add(del))
        })
    });
    let icon_color = if tool.status == ToolStatus::Failed {
        t.danger
    } else if running {
        t.muted
    } else {
        t.faint
    };
    let header_key = format!("{}-row", tool.key);
    let mut label_el = text(label.clone())
        .font(if is_command(&tool.name) {
            t.mono_small
        } else {
            t.ui
        })
        .color(t.text_secondary)
        .group_color(t.text)
        .shrink()
        .min_w(0.0)
        .truncate();
    label_el = match tooltip {
        Some(tip) => label_el.help(tip),
        None => label_el.help_when_truncated(label.clone()),
    };
    let lead = row()
        .gap(S6)
        .center_y()
        .shrink()
        .min_w(0.0)
        .child(icon_slot(
            icon(tool_icon(tool, action), ICON)
                .color(icon_color)
                .group_color(if tool.status == ToolStatus::Failed {
                    t.danger
                } else {
                    t.text
                }),
        ))
        .child(label_el);
    let lead = motion::text_shimmer(
        format!("{}-shimmer", tool.key),
        running,
        lead,
        rune::Color::WHITE,
        if t.k.dark { 0.9 } else { 0.62 },
    );
    let mut head = row()
        .key(header_key.clone())
        .w_full()
        .min_h(S24)
        .px(S4)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .child(lead)
        .child(rune::spacer());

    if let Some((add, del)) = diff_totals {
        head = head.child(ui::diff_stat(add, del, t));
    }
    if let Some(status_el) = tool_status_meta(tool, s.model.disclosure.tool_clock, t) {
        head = head.child(status_el);
    }
    if let Some(clock) = tool.ended_at.or(tool.started_at).and_then(format_clock_hm) {
        head = head.child(
            text(clock)
                .font(t.micro)
                .color(t.faint)
                .group_opacity(0.0, 1.0),
        );
    }
    if expandable {
        head = make_foldable(head, tool.key.clone(), header_key, label, open, t);
    }
    head
}

fn make_foldable(
    head: El,
    toggle_key: String,
    anchor: String,
    label: String,
    open: bool,
    t: &Theme,
) -> El {
    head.role("button")
        .label(label)
        .expanded(open)
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::ToggleFold {
                    key: toggle_key.clone(),
                    legacy_key: None,
                    default_open: false,
                    anchor: Some(anchor.clone()),
                }),
                tx,
            );
        })
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .key("chevron")
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

/// Fixed `24×24pt` centered leading icon slot matching `timeline.md` §4B.
pub fn icon_slot(glyph: El) -> El {
    el().size(S24, S24).center().a11y_hidden().child(glyph)
}

/// Maps a tool call to its stroke-2 Lucide activity glyph.
pub fn tool_icon(tool: &Tool, action: ToolAction) -> &'static Icon {
    if tool.viz.is_some() {
        return &ui::BOT;
    }
    match action {
        ToolAction::Read => &ui::EYE,
        ToolAction::Edit | ToolAction::Write | ToolAction::Move => &ui::COMPOSE,
        ToolAction::Delete => &ui::TRASH,
        ToolAction::Command => &ui::TERMINAL,
        ToolAction::Search => &ui::SEARCH,
        ToolAction::Fetch => &ui::GLOBE,
        ToolAction::Other => &ui::WRENCH,
    }
}

fn tool_display_label(s: &Desktop, tool: &Tool, action: ToolAction) -> (String, Option<String>) {
    let detail = tool.summary.trim();
    if is_command(&tool.name) {
        let cmd = detail.strip_prefix("$ ").unwrap_or(detail);
        let tip = tool
            .input
            .as_deref()
            .filter(|full| *full != cmd)
            .map(str::to_owned);
        let label = if cmd.is_empty() {
            "Ran command".to_owned()
        } else {
            cmd.to_owned()
        };
        return (label, tip);
    }
    if action.on_file() && !detail.is_empty() {
        let rel = relative_path(&s.model, detail);
        let label = match action {
            ToolAction::Read => format!("Read {rel}"),
            ToolAction::Delete => format!("Deleted {rel}"),
            _ => rel.to_owned(),
        };
        return (label, Some(detail.to_owned()));
    }
    if action == ToolAction::Search && !detail.is_empty() {
        return (format!("Searched for {detail}"), None);
    }
    if action == ToolAction::Fetch && !detail.is_empty() {
        return (format!("Fetched {detail}"), None);
    }
    let name = tool
        .raw_name
        .as_deref()
        .filter(|raw| !raw.is_empty())
        .unwrap_or(&tool.name);
    let human = if matches!(name, "tool" | "other" | "") {
        tool.title.clone()
    } else {
        humanize_tool(name)
    };
    if detail.is_empty() || detail == human || detail == name {
        (human, None)
    } else {
        (format!("{human} {detail}"), None)
    }
}

/// Trailing live spinner, timeout ring, or failure status tag on a tool row.
pub fn tool_status_meta(tool: &Tool, now: i64, t: &Theme) -> Option<El> {
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
        ToolStatus::Pending => Some(text("Queued").font(t.micro).color(t.faint)),
        ToolStatus::Abandoned => Some(text("Stopped").font(t.micro).color(t.faint)),
        ToolStatus::Failed => {
            let msg = tool
                .exit_code
                .filter(|&code| code != 0)
                .map_or_else(|| "Failed".to_owned(), |code| format!("Exit {code}"));
            Some(text(msg).font(t.micro).color(t.danger))
        }
        ToolStatus::Done => None,
    }
}
