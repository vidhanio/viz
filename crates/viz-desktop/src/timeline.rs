//! The timeline: user bubbles, assistant prose as Markdown, thinking and tool
//! calls as collapsible work-log rows, turn dividers, and the live answer as it
//! streams in. It sticks to the bottom while new content arrives.

use rune::{Align, El, Icon, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Collapsible, ControlSize, Kit, Markdown, Spinner};
use rune_theme_opencode::icons;

use crate::{
    app::{Desktop, Status},
    theme,
    thread::{Item, Origin, Tool, ToolStatus, User},
};

/// The widest a user bubble gets.
const BUBBLE_WIDTH: f32 = 560.0;
/// The tallest an expanded tool block gets before it scrolls.
const BLOCK_HEIGHT: f32 = 280.0;

/// The scrolling timeline, or what to show in its place.
pub fn timeline(s: &Desktop, k: &Kit) -> El {
    let thread = &s.thread;
    let empty = thread.items.is_empty() && !thread.live.busy && thread.live.failure.is_none();
    let body = if empty {
        placeholder(s, k)
    } else {
        let mut rows = Vec::new();
        if thread.earlier {
            rows.push(
                text("Earlier messages aren't shown")
                    .key("earlier")
                    .font(k.ty.small)
                    .color(k.c.text_faint)
                    .self_center(),
            );
        }
        items(s, &thread.items, false, &mut rows, k);
        live(s, &mut rows, k);
        col()
            .key("items")
            .w_full()
            .max_w(theme::READING_WIDTH)
            .gap(16.0)
            .children(rows)
    };
    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(24.0)
        .child(
            col()
                .key("timeline-body")
                .w_full()
                .min_h(0.0)
                .px(24.0)
                .pt(20.0)
                .pb(28.0)
                .items_center()
                .child(body),
        )
}

/// Builds `items` into `rows`, gathering consecutive tool calls into one
/// work-log card. `streaming` marks the in-flight answer.
fn items(s: &Desktop, items: &[Item], streaming: bool, rows: &mut Vec<El>, k: &Kit) {
    let mut tools: Vec<&Tool> = Vec::new();
    let flush = |tools: &mut Vec<&Tool>, rows: &mut Vec<El>| {
        if !tools.is_empty() {
            rows.push(work_log(s, tools, k));
            tools.clear();
        }
    };
    for item in items {
        if let Item::Tool(tool) = item {
            tools.push(tool);
            continue;
        }
        flush(&mut tools, rows);
        rows.push(match item {
            Item::User(user) => bubble(user, k),
            Item::Text { key, text } => Markdown::new(text.clone())
                .streaming(streaming)
                .selectable(true)
                .on_link_tx(|_: &mut Desktop, url: &str, tx: &mut Tx| {
                    _ = tx.open_url(url);
                })
                .build(k)
                .key(key.clone())
                .w_full(),
            Item::Thinking { key, text } => thinking(s, key, text, k),
            Item::Divider { key, label } => divider(key, label, k),
            Item::Notice { key, text } => notice(key, text, k),
            Item::Tool(_) => unreachable!("tools are gathered above"),
        });
    }
    flush(&mut tools, rows);
}

/// What's happening now: the streaming answer, a working line, a retry or
/// compaction, or why the last run ended without an answer.
fn live(s: &Desktop, rows: &mut Vec<El>, k: &Kit) {
    let live = &s.thread.live;
    let c = &k.c;
    items(s, &live.partial, true, rows, k);
    if live.busy {
        let running = s.thread.items.iter().rev().find_map(|item| match item {
            Item::Tool(tool) if tool.status == ToolStatus::Running => Some(tool.name.as_str()),
            _ => None,
        });
        let label = match (&live.retry, running, live.compacting) {
            (Some(error), ..) => format!("Retrying after an error: {error}"),
            (None, Some(tool), _) => format!("Running {tool}…"),
            (None, None, true) => "Compacting context…".to_owned(),
            (None, None, false) if live.partial.is_empty() => "Thinking…".to_owned(),
            (None, None, false) => "Writing…".to_owned(),
        };
        rows.push(
            row()
                .key("working")
                .role("status")
                .label(label.clone())
                .gap(8.0)
                .center_y()
                .child(Spinner::new().size(14.0).build(k))
                .child(
                    text(label)
                        .font(k.ty.ui)
                        .color(if live.retry.is_some() {
                            c.warning.fg
                        } else {
                            c.text_muted
                        })
                        .truncate()
                        .shrink(),
                ),
        );
    }
    if let Some(failure) = &live.failure {
        rows.push(notice("run-failure", failure, k));
    }
}

/// The timeline before there's anything in it.
fn placeholder(s: &Desktop, k: &Kit) -> El {
    let c = &k.c;
    let (glyph, title, line) = match (&s.status, s.selected) {
        (Status::Error(_), _) => (
            &icons::OUTLINE_HEXAGONAL_WARNING,
            "Not connected".to_owned(),
            "The desktop app talks to viz-backend. Start it, or point [client].url at a running one; this window reconnects on its own.",
        ),
        (Status::Connecting, _) | (_, None) => (
            &icons::SERVER,
            "Connecting…".to_owned(),
            "Looking for the backend.",
        ),
        (Status::Connected { .. }, Some(id)) if Some(id) == s.root => (
            &icons::OUTLINE_BRAIN,
            "Orchestrator".to_owned(),
            "One conversation for everything: messages from here and from connectors land in it, and it hands work to subagents.",
        ),
        (Status::Connected { .. }, Some(id)) => (
            &icons::SPEECH_BUBBLE,
            s.title(id),
            "A fresh conversation. Ask anything.",
        ),
    };
    col()
        .key("placeholder")
        .w_full()
        .max_w(420.0)
        .pt(96.0)
        .gap(10.0)
        .items_center()
        .child(
            el().size(44.0, 44.0)
                .radius(12.0)
                .center()
                .fill(c.layer_02)
                .border(1.0, c.border_weak)
                .child(icon(glyph, 22.0).color(c.icon_muted)),
        )
        .child(
            text(title)
                .key("placeholder-title")
                .font(k.ty.body_medium)
                .color(c.text_base),
        )
        .child(
            text(line)
                .key("placeholder-line")
                .font(k.ty.ui)
                .color(c.text_muted)
                .wrap()
                .align(Align::Center)
                .w_full(),
        )
}

/// A user's message: a bubble on the right, with a chip naming the connector
/// it came through.
fn bubble(user: &User, k: &Kit) -> El {
    let c = &k.c;
    let mut column = col().key(user.key.clone()).w_full().gap(4.0).items_end();
    if let Some(origin) = &user.origin {
        column = column.child(origin_chip(origin, k));
    }
    column.child(
        el().key("bubble")
            .max_w(BUBBLE_WIDTH)
            .px(14.0)
            .py(9.0)
            .radius(k.radius.xl)
            .fill(c.layer_02)
            .border(1.0, c.border_weak)
            .child(
                text(user.text.clone())
                    .font(k.ty.body)
                    .color(c.text_base)
                    .wrap()
                    .selectable(true)
                    .selection_color(k.selection()),
            ),
    )
}

/// `discord · @alice`, with the connector's mark when there is one.
fn origin_chip(origin: &Origin, k: &Kit) -> El {
    let c = &k.c;
    let glyph = match origin.connector.as_str() {
        "discord" => &icons::DISCORD,
        "github" => &icons::GITHUB,
        _ => &icons::SPEECH_BUBBLE,
    };
    let who = origin.author.as_ref().map_or_else(
        || format!("#{}", origin.channel),
        |author| format!("@{author}"),
    );
    row()
        .key("origin")
        .h(20.0)
        .px(6.0)
        .gap(4.0)
        .center_y()
        .radius(k.radius.sm)
        .fill(c.layer_01)
        .border(1.0, c.border_weak)
        .help(format!(
            "From {} · channel {}",
            origin.connector, origin.channel
        ))
        .child(icon(glyph, 12.0).color(c.icon_muted))
        .child(
            text(format!("{} · {who}", origin.connector))
                .font(k.ty.small_medium)
                .color(c.text_muted),
        )
}

fn thinking(s: &Desktop, key: &str, thought: &str, k: &Kit) -> El {
    let open = s.expanded.contains(key);
    let body = if thought.trim().is_empty() {
        "The provider redacted this reasoning."
    } else {
        thought
    };
    let toggle = key.to_owned();
    Collapsible::new("Thinking", open)
        .ghost()
        .child(
            el().key("thought")
                .w_full()
                .pl(12.0)
                .ml(4.0)
                .border_sides([0.0, 0.0, 0.0, 2.0], k.c.border_weak)
                .child(
                    text(body.to_owned())
                        .font(k.ty.text_small)
                        .color(k.c.text_muted)
                        .wrap()
                        .selectable(true)
                        .selection_color(k.selection()),
                ),
        )
        .on_toggle(move |s: &mut Desktop| s.toggle(&toggle))
        .build(k)
        .key(key.to_owned())
}

/// A run of tool calls: one card, a row each.
fn work_log(s: &Desktop, tools: &[&Tool], k: &Kit) -> El {
    let c = &k.c;
    let key = format!("work-{}", tools[0].key);
    let rows = tools.iter().enumerate().map(|(i, tool)| {
        let row = tool_row(s, tool, k);
        if i == 0 {
            row
        } else {
            row.border_sides([1.0, 0.0, 0.0, 0.0], c.border_weak)
        }
    });
    col()
        .key(key)
        .role("list")
        .label("Tool calls")
        .w_full()
        .radius(k.radius.lg)
        .fill(c.layer_01)
        .border(1.0, c.border_weak)
        .clip_rounded(k.radius.lg)
        .children(rows)
}

fn tool_row(s: &Desktop, tool: &Tool, k: &Kit) -> El {
    let c = &k.c;
    let open = s.expanded.contains(&tool.key);
    let toggle = tool.key.clone();
    let failed = tool.status == ToolStatus::Failed;
    let mut header = row()
        .key("header")
        .role("button")
        .label(format!("{} {}", tool.name, tool.summary))
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, -2.0, c.border_focus)
        .w_full()
        .h(34.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .on_press(move |s: &mut Desktop| s.toggle(&toggle))
        .child(status_icon(tool, k))
        .child(
            text(tool.name.clone())
                .key("name")
                .font(k.ty.ui_medium)
                .color(if failed { c.danger.fg } else { c.text_base }),
        )
        .child(
            text(tool.summary.clone())
                .key("summary")
                .font(k.ty.ui)
                .color(c.text_muted)
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    if let Some(child) = tool.child {
        header = header.child(
            Button::new(format!("Open #{child}"))
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .icon(&icons::ARROW_UP_RIGHT)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| s.select(child, tx))
                .build(k)
                .key("open-child"),
        );
    }
    let meta = match tool.status {
        ToolStatus::Pending => Some("queued".to_owned()),
        ToolStatus::Running => Some("running".to_owned()),
        ToolStatus::Abandoned => Some("no result".to_owned()),
        ToolStatus::Done | ToolStatus::Failed => tool.duration_ms.map(theme::duration),
    };
    if let Some(meta) = meta {
        header = header.child(text(meta).key("meta").font(k.ty.small).color(c.text_faint));
    }
    header = k.interactive(
        header.child(
            icon(
                if open {
                    k.glyph.chevron_up
                } else {
                    k.glyph.chevron_down
                },
                14.0,
            )
            .color(c.icon_faint),
        ),
    );
    let mut tool_el = col().key(tool.key.clone()).w_full().child(header);
    if open {
        let mut body = col()
            .key("body")
            .w_full()
            .px(10.0)
            .pt(2.0)
            .pb(10.0)
            .gap(8.0);
        if !tool.arguments.is_empty() && tool.arguments != "{}" {
            body = body.child(block("arguments", "Arguments", &tool.arguments, false, k));
        }
        let (label, empty) = match tool.status {
            ToolStatus::Failed => ("Error", "No error text"),
            ToolStatus::Running | ToolStatus::Pending => ("Output", "No output yet"),
            ToolStatus::Done => ("Result", "No output"),
            ToolStatus::Abandoned => ("Result", "The run ended before this call finished"),
        };
        let output = if tool.output.is_empty() {
            empty
        } else {
            tool.output.as_str()
        };
        body = body.child(block("output", label, output, failed, k));
        tool_el = tool_el.child(body);
    }
    tool_el
}

/// A tool row's leading mark: its status while it runs or when it failed,
/// else the tool's icon.
fn status_icon(tool: &Tool, k: &Kit) -> El {
    let c = &k.c;
    let glyph = |glyph: &'static Icon, color| icon(glyph, 14.0).color(color);
    let mark = match tool.status {
        ToolStatus::Running => Spinner::new().size(14.0).build(k),
        ToolStatus::Pending => theme::dot(c.icon_faint, 6.0),
        ToolStatus::Failed => glyph(&icons::CIRCLE_X, c.danger.icon),
        ToolStatus::Abandoned => glyph(&icons::CIRCLE_BAN_SIGN, c.icon_faint),
        ToolStatus::Done => glyph(theme::tool_icon(&tool.name), c.icon_muted),
    };
    el().key("status").size(16.0, 16.0).center().child(mark)
}

/// A labelled monospace block that scrolls past [`BLOCK_HEIGHT`].
fn block(key: &str, label: &str, body: &str, error: bool, k: &Kit) -> El {
    let c = &k.c;
    col()
        .key(key.to_owned())
        .w_full()
        .gap(4.0)
        .child(
            text(label.to_owned())
                .font(k.ty.small_medium)
                .color(c.text_faint),
        )
        .child(
            col()
                .key("scroll")
                .w_full()
                .max_h(BLOCK_HEIGHT)
                .radius(k.radius.md)
                .fill(c.bg_base)
                .border(1.0, c.border_weak)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    text(body.to_owned())
                        .font(k.ty.mono)
                        .color(if error { c.danger.fg } else { c.text_base })
                        .wrap_anywhere()
                        .selectable(true)
                        .selection_color(k.selection())
                        .w_full()
                        .padding(8.0),
                ),
        )
}

/// A turn boundary: a thin rule with a quiet label.
fn divider(key: &str, label: &str, k: &Kit) -> El {
    let c = &k.c;
    let rule = || el().h(1.0).grow(1.0).fill(c.border_weak);
    row()
        .key(key.to_owned())
        .role("separator")
        .label(label.to_owned())
        .w_full()
        .gap(10.0)
        .center_y()
        .child(rule())
        .child(
            text(label.to_owned())
                .font(k.ty.small_medium)
                .color(c.text_faint),
        )
        .child(rule())
}

/// An answer that didn't come: a danger-tinted line.
fn notice(key: &str, message: &str, k: &Kit) -> El {
    let danger = &k.c.danger;
    row()
        .key(key.to_owned())
        .role("alert")
        .w_full()
        .px(12.0)
        .py(8.0)
        .gap(8.0)
        .items_start()
        .radius(k.radius.md)
        .fill(danger.bg)
        .border(1.0, danger.border)
        .child(
            el().size(16.0, 20.0)
                .center()
                .child(icon(&icons::CIRCLE_EXCLAMATION, 14.0).color(danger.icon)),
        )
        .child(
            text(message.to_owned())
                .font(k.ty.ui)
                .color(danger.fg)
                .wrap()
                .selectable(true)
                .grow(1.0)
                .shrink(),
        )
}
