//! The timeline: one centred reading column of full-width rows. My messages
//! sit on a faint accent wash behind the tide line; the assistant's answers
//! are plain Markdown; tool calls, subagent calls and reasoning are compact
//! rows that stack tightly between them; turns are set apart by space and a
//! short rule. The live answer streams in with a caret. It sticks to the
//! bottom while new content arrives.
//!
//! rune-kit has no compact log row, so the message and tool rows are drawn
//! here; prose is the kit's `Markdown`, reasoning the kit's `Collapsible`,
//! and the busy marks its `Spinner`.

use rune::{Align, El, LoopSpec, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Collapsible, ControlSize, Markdown, Spinner};

use crate::{
    app::{Desktop, Status},
    theme::{self, Theme},
    thread::{Item, Tool, ToolStatus, User},
};

/// The tallest an expanded tool block gets before it scrolls.
const BLOCK_HEIGHT: f32 = 280.0;
/// A tool row's height.
const TOOL_ROW: f32 = 26.0;

/// What a row is, for the space above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mine,
    Prose,
    Activity,
    Turn,
    Notice,
}

/// Points above a row of kind `next` that follows one of kind `prev`.
const fn space(prev: Kind, next: Kind) -> f32 {
    match (prev, next) {
        (Kind::Turn, _) | (_, Kind::Turn) => 0.0,
        (Kind::Activity, Kind::Activity) => 1.0,
        (Kind::Mine, _) => 20.0,
        (_, Kind::Mine) => 28.0,
        _ => 14.0,
    }
}

/// Rows in order, each spaced from the one before by its kind.
struct Rows {
    rows: Vec<El>,
    last: Option<Kind>,
}

impl Rows {
    fn push(&mut self, kind: Kind, el: El) {
        let gap = self.last.map_or(0.0, |prev| space(prev, kind));
        self.rows.push(el.mt(gap));
        self.last = Some(kind);
    }
}

/// The scrolling timeline, or what to show in its place.
pub fn timeline(s: &Desktop, t: &Theme) -> El {
    let thread = &s.thread;
    let empty = thread.items.is_empty() && !thread.live.busy && thread.live.failure.is_none();
    let body = if empty {
        empty_state(s, t)
    } else {
        let mut rows = Rows {
            rows: Vec::new(),
            last: None,
        };
        if thread.earlier {
            rows.push(
                Kind::Notice,
                text("Earlier messages aren't shown")
                    .key("earlier")
                    .font(t.small)
                    .color(t.faint)
                    .self_center(),
            );
        }
        items(s, &thread.items, false, &mut rows, t);
        live(s, &mut rows, t);
        col()
            .key("items")
            .w_full()
            .max_w(theme::READING_WIDTH)
            .children(rows.rows)
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
                .pt(28.0)
                .pb(24.0)
                .items_center()
                .child(body),
        )
}

/// Builds `items` into `rows`. `streaming` marks the in-flight answer.
fn items(s: &Desktop, items: &[Item], streaming: bool, rows: &mut Rows, t: &Theme) {
    for item in items {
        match item {
            Item::User(user) => rows.push(Kind::Mine, mine(user, t)),
            Item::Text { key, text } => rows.push(Kind::Prose, prose(key, text, streaming, t)),
            Item::Thinking {
                key,
                text,
                duration_ms,
            } => rows.push(
                Kind::Activity,
                thinking(s, key, text, *duration_ms, streaming, t),
            ),
            Item::Tool(tool) => rows.push(
                Kind::Activity,
                if tool.child.is_some() || tool.name == "subagent" {
                    subagent_row(s, tool, t)
                } else {
                    tool_row(s, tool, t)
                },
            ),
            Item::Divider { key, label } => rows.push(Kind::Turn, divider(key, *label, t)),
            Item::Notice { key, text } => rows.push(Kind::Notice, notice(key, text, t)),
        }
    }
}

/// What's happening now: the streaming answer and its caret, a quiet
/// working line before there's anything to show, a retry or compaction, or
/// why the last run ended without an answer.
fn live(s: &Desktop, rows: &mut Rows, t: &Theme) {
    let live = &s.thread.live;
    items(s, &live.partial, true, rows, t);
    if live.busy {
        let running = s
            .thread
            .items
            .iter()
            .any(|item| matches!(item, Item::Tool(tool) if tool.status == ToolStatus::Running));
        let writing = matches!(live.partial.last(), Some(Item::Text { .. }));
        let status = match (&live.retry, live.compacting) {
            (Some(error), _) => Some((format!("Retrying after an error: {error}"), t.warning)),
            (None, true) => Some(("Compacting context…".to_owned(), t.muted)),
            (None, false) if live.partial.is_empty() && !running => {
                Some(("Thinking…".to_owned(), t.faint))
            }
            (None, false) => None,
        };
        if let Some((label, color)) = status {
            rows.push(
                Kind::Activity,
                row()
                    .key("working")
                    .role("status")
                    .label(label.clone())
                    .h(TOOL_ROW)
                    .px(6.0)
                    .gap(8.0)
                    .center_y()
                    .child(
                        el().size(12.0, 12.0)
                            .center()
                            .child(theme::pulse(t.accent, 6.0)),
                    )
                    .child(text(label).font(t.ui).color(color).truncate().shrink()),
            );
        } else if writing {
            rows.push(
                Kind::Activity,
                el().key("caret")
                    .w(2.0)
                    .h(17.0)
                    .radius(1.0)
                    .fill(t.accent)
                    .looping(LoopSpec::opacity(1.0, 0.0, 0.0, 1.0))
                    .a11y_hidden(),
            );
        }
    }
    if let Some(failure) = &live.failure {
        rows.push(Kind::Notice, notice("run-failure", failure, t));
    }
}

/// The timeline before there's anything in it: the wordmark and a line.
fn empty_state(s: &Desktop, t: &Theme) -> El {
    let (line, retry) = match (&s.status, s.selected) {
        (Status::Error(_), _) => (
            "Can't reach the backend. Start viz-backend; this window reconnects on its own.",
            true,
        ),
        (Status::Connecting, _) | (_, None) => ("Connecting…", false),
        (Status::Connected { .. }, Some(_)) => ("What's on your mind?", false),
    };
    let mark = row()
        .key("wordmark")
        .gap(3.0)
        .items_end()
        .child(
            text("viz")
                .font(rune::TextStyle::new(30.0, 640.0).lh(34.0).tracking(-0.8))
                .color(t.text),
        )
        .child(theme::dot(t.accent, 7.0).mb(8.0).shadow(0.0, 10.0, t.glow));
    let mut state = col()
        .key("empty")
        .w_full()
        .max_w(420.0)
        .pt(140.0)
        .gap(10.0)
        .items_center()
        .child(mark)
        .child(
            text(line)
                .key("empty-line")
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(Align::Center)
                .w_full(),
        );
    if retry {
        state = state.child(
            Button::new("Retry now")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut Desktop, tx: &mut Tx| s.reconnect(tx))
                .build(&t.k)
                .key("retry"),
        );
    }
    state
}

/// Something I wrote: a full-width row on a faint wash, the tide line at its
/// edge.
fn mine(user: &User, t: &Theme) -> El {
    el().key(user.key.clone())
        .role("article")
        .label("You")
        .w_full()
        .px(16.0)
        .py(11.0)
        .radius(t.k.radius.md)
        .fill(t.mine)
        .child(
            text(user.text.clone())
                .font(t.body)
                .color(t.text)
                .wrap()
                .w_full()
                .selectable(true)
                .selection_color(t.k.selection()),
        )
        .child(
            el().w(2.0)
                .pin(Some(0.0), Some(9.0), None, Some(9.0))
                .radius(1.0)
                .fill(t.accent)
                .shadow(0.0, 8.0, t.glow)
                .a11y_hidden(),
        )
}

/// Assistant prose.
fn prose(key: &str, source: &str, streaming: bool, t: &Theme) -> El {
    Markdown::new(source.to_owned())
        .style(t.markdown())
        .streaming(streaming)
        .selectable(true)
        .on_link_tx(|_: &mut Desktop, url: &str, tx: &mut Tx| {
            _ = tx.open_url(url);
        })
        .build(&t.k)
        .key(key.to_owned())
        .w_full()
}

/// Reasoning: one faint collapsed row.
fn thinking(
    s: &Desktop,
    key: &str,
    thought: &str,
    duration_ms: Option<u64>,
    streaming: bool,
    t: &Theme,
) -> El {
    let open = s.expanded.contains(key);
    let title = match duration_ms {
        _ if streaming => "Thinking…".to_owned(),
        Some(ms) => format!("Thought for {}s", (ms + 500) / 1000),
        None => "Thought".to_owned(),
    };
    let body = if thought.trim().is_empty() {
        "The provider redacted this reasoning."
    } else {
        thought
    };
    let toggle = key.to_owned();
    Collapsible::new(title, open)
        .ghost()
        .child(
            el().key("thought")
                .w_full()
                .ml(6.0)
                .mb(6.0)
                .pl(12.0)
                .border_sides([0.0, 0.0, 0.0, 1.0], t.hairline)
                .child(
                    text(body.to_owned())
                        .font(t.ui)
                        .color(t.muted)
                        .wrap()
                        .selectable(true)
                        .selection_color(t.k.selection()),
                ),
        )
        .on_toggle(move |s: &mut Desktop| s.toggle(&toggle))
        .build(&t.k)
        .key(key.to_owned())
        .pl(6.0)
}

/// What a settled or running call says at its right end.
fn meta(tool: &Tool, t: &Theme) -> Option<El> {
    let faint = |words: String| text(words).font(t.mono_small).color(t.faint);
    match tool.status {
        ToolStatus::Running => Some(Spinner::new().size(12.0).color(t.accent).build(&t.k)),
        ToolStatus::Pending => Some(faint("queued".to_owned())),
        ToolStatus::Abandoned => Some(faint("no result".to_owned())),
        ToolStatus::Done | ToolStatus::Failed => {
            tool.duration_ms.map(|ms| faint(theme::duration(ms)))
        }
    }
}

/// A tool call: chevron, name, a faint summary of its arguments, its time.
/// Opens into its arguments and output.
fn tool_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let open = s.expanded.contains(&tool.key);
    let toggle = tool.key.clone();
    let failed = tool.status == ToolStatus::Failed;
    let mut header = row()
        .key("header")
        .role("button")
        .label(format!("{} {}", tool.name, tool.summary))
        .expanded(open)
        .focusable(true)
        .focus_ring(2.0, -2.0, t.k.c.border_focus)
        .w_full()
        .h(TOOL_ROW)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(t.k.radius.md)
        .on_press(move |s: &mut Desktop| s.toggle(&toggle))
        .child(
            icon(
                if open {
                    &theme::CHEVRON_OPEN
                } else {
                    &theme::CHEVRON
                },
                12.0,
            )
            .color(t.faint),
        )
        .child(
            text(tool.name.clone())
                .key("name")
                .font(t.mono_medium)
                .color(if failed { t.danger } else { t.muted }),
        )
        .child(
            text(tool.summary.clone())
                .key("summary")
                .font(t.mono)
                .color(t.faint)
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    if let Some(meta) = meta(tool, t) {
        header = header.child(meta.key("meta"));
    }
    let header = header.hover(t.hover);
    let mut el = col().key(tool.key.clone()).w_full().child(header);
    if open {
        el = el.child(output(tool, t));
    }
    el
}

/// An open tool row's arguments and output, in a scrolling mono block.
fn output(tool: &Tool, t: &Theme) -> El {
    let failed = tool.status == ToolStatus::Failed;
    let empty = match tool.status {
        ToolStatus::Failed => "No error text",
        ToolStatus::Running | ToolStatus::Pending => "No output yet",
        ToolStatus::Done => "No output",
        ToolStatus::Abandoned => "The run ended before this call finished",
    };
    let mut inner = col().key("inner").w_full().padding(12.0).gap(10.0);
    if !tool.arguments.is_empty() && tool.arguments != "{}" {
        inner = inner.child(
            text(tool.arguments.clone())
                .key("arguments")
                .font(t.mono)
                .color(t.faint)
                .wrap_anywhere()
                .selectable(true)
                .selection_color(t.k.selection())
                .w_full(),
        );
        inner = inner.child(el().w_full().h(1.0).fill(t.hairline));
    }
    let body = if tool.output.is_empty() {
        (empty.to_owned(), t.faint)
    } else {
        (tool.output.clone(), if failed { t.danger } else { t.text })
    };
    inner = inner.child(
        text(body.0)
            .key("output")
            .font(t.mono)
            .color(body.1)
            .wrap_anywhere()
            .selectable(true)
            .selection_color(t.k.selection())
            .w_full(),
    );
    col()
        .key("body")
        .w_full()
        .mt(2.0)
        .mb(6.0)
        .ml(26.0)
        .max_h(BLOCK_HEIGHT)
        .radius(t.k.radius.md)
        .fill(t.surface)
        .border(1.0, t.hairline)
        .clip_rounded(t.k.radius.md)
        .scroll(ScrollSpec {
            stick_bottom: tool.status == ToolStatus::Running,
        })
        .child(inner)
        .shrink()
}

/// A subagent call: "↳ title", opening the child conversation.
fn subagent_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let title = tool
        .child
        .filter(|child| {
            s.summary(*child)
                .is_some_and(|summary| !summary.title.is_empty())
        })
        .map_or_else(|| tool.summary.clone(), |child| s.title(child));
    let failed = tool.status == ToolStatus::Failed;
    let mut el = row()
        .key(tool.key.clone())
        .role("link")
        .label(format!("Subagent: {title}"))
        .w_full()
        .h(TOOL_ROW + 2.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(t.k.radius.md)
        .child(icon(&theme::BRANCH, 13.0).color(if failed { t.danger } else { t.accent }))
        .child(
            text(title)
                .key("title")
                .font(t.ui)
                .color(t.text)
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    if let Some(meta) = meta(tool, t) {
        el = el.child(meta.key("meta"));
    }
    match tool.child {
        Some(child) => el
            .focusable(true)
            .focus_ring(2.0, -2.0, t.k.c.border_focus)
            .help("Open the subagent's conversation")
            .hover(t.hover)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| s.select(child, tx)),
        None => el,
    }
}

/// A turn boundary: room, and a short faint rule; or a quiet label for
/// other markers.
fn divider(key: &str, label: Option<&str>, t: &Theme) -> El {
    let mark = label.map_or_else(
        || el().w(40.0).h(1.0).fill(t.hairline),
        |label| text(label.to_owned()).font(t.small).color(t.faint),
    );
    row()
        .key(key.to_owned())
        .role("separator")
        .label(label.unwrap_or("New turn").to_owned())
        .w_full()
        .py(22.0)
        .center()
        .child(mark)
}

/// An answer that didn't come: a danger-tinted line.
fn notice(key: &str, message: &str, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .role("alert")
        .w_full()
        .px(6.0)
        .gap(10.0)
        .items_start()
        .child(
            el().size(12.0, 20.0)
                .center()
                .child(theme::dot(t.danger, 6.0)),
        )
        .child(
            text(message.to_owned())
                .font(t.ui)
                .color(t.danger)
                .wrap()
                .selectable(true)
                .grow(1.0)
                .shrink(),
        )
}
