//! Transcript, activity, plan checklists, and expandable tool records.

use rune::{El, LoopSpec, ScrollSpec, Span, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    ButtonVariant, Collapsible, Markdown,
    diff::{FileDiff, LineKind},
};

use crate::{
    app::{Desktop, Status},
    theme::{self, Theme},
    thread::{Item, PlanStep, Tool, ToolStatus, User},
    ui,
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
    let empty =
        thread.items.is_empty() && !s.busy() && !thread.live.busy && thread.live.failure.is_none();
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
            Item::Plan { key, steps } => rows.push(Kind::Activity, plan(key, steps, t)),
            Item::Progress { key, text: words } => rows.push(
                Kind::Activity,
                text(words)
                    .key(key)
                    .font(t.ui)
                    .color(t.muted)
                    .wrap()
                    .px(6.0),
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
    if s.busy() || live.busy {
        let running = s
            .thread
            .items
            .iter()
            .any(|item| matches!(item, Item::Tool(tool) if tool.status == ToolStatus::Running));
        let writing = matches!(live.partial.last(), Some(Item::Text { .. }));
        let status = match (&live.retry, live.compacting) {
            (Some(error), _) => Some((format!("Retrying: {error}"), t.warning)),
            (None, true) => Some(("Compacting context".to_owned(), t.muted)),
            (None, false) if live.partial.is_empty() && !running => {
                let label = if s
                    .conversation
                    .as_ref()
                    .is_some_and(|state| !state.approvals.is_empty())
                {
                    "Waiting for approval"
                } else {
                    "Working"
                };
                Some((label.to_owned(), t.muted))
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
                    .child(ui::spinner(12.0, t))
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

/// What to show when the timeline has no messages.
fn empty_state(s: &Desktop, t: &Theme) -> El {
    let (title, detail, retry) = match (&s.status, s.selected, &s.new_chat) {
        (Status::Error(_), _, _) => ("Offline", "Start viz-backend to reconnect.", true),
        (Status::Connecting, _, _) => ("Connecting", "", false),
        (_, _, Some(chat)) => (
            "New chat",
            if chat.request.is_some() {
                "Opening session."
            } else if s.harnesses.len() > 1 {
                "Choose a harness below."
            } else {
                ""
            },
            false,
        ),
        (_, Some(_), None) => (
            "Start a conversation",
            "Ask a question or describe a task.",
            false,
        ),
        (_, None, None) => ("No chat selected", "", false),
    };
    let mut state = ui::empty(title, detail, t).pt(132.0);
    if retry {
        state = state.child(
            ui::button("Retry", ButtonVariant::Outline)
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry")
                .mt(4.0),
        );
    }
    state
}

/// A user message sits on a neutral surface.
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
        "Reasoning hidden"
    } else {
        thought
    };
    let toggle = key.to_owned();
    Collapsible::new(title, open)
        .ghost()
        .child(
            text(body)
                .key("thought")
                .ml(6.0)
                .mb(6.0)
                .px(12.0)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .selectable(true)
                .selection_color(t.k.selection()),
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
        ToolStatus::Running => Some(ui::spinner(12.0, t)),
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
    let mut inner = col().key("inner").w_full().gap(10.0).padding(10.0);
    if !tool.arguments.is_empty() && tool.arguments != "{}" {
        inner = inner.child(code_section("Input", &tool.arguments, t));
    }
    if !tool.output.is_empty() {
        inner = inner.child(code_section("Output", &tool.output, t));
    }
    if !tool.locations.is_empty() {
        inner = inner.child(code_section("Paths", &tool.locations.join("\n"), t));
    }
    if !tool.diffs.is_empty() {
        inner = inner.child(rune::memo(
            format!("{}-diffs", tool.key),
            (&tool.diffs, t.k.dark),
            || diff_sections(tool, t),
        ));
    }
    if tool.arguments.trim_matches([' ', '{', '}']).is_empty()
        && tool.output.is_empty()
        && tool.locations.is_empty()
        && tool.diffs.is_empty()
    {
        inner = inner.child(ui::section("No details", t));
    }
    col()
        .key("body")
        .w_full()
        .mt(2.0)
        .mb(7.0)
        .max_h(BLOCK_HEIGHT)
        .min_h(0.0)
        .radius(t.k.radius.md)
        .fill(t.surface)
        .border(1.0, t.hairline)
        .clip_rounded(t.k.radius.md)
        .scroll(ScrollSpec {
            stick_bottom: tool.status == ToolStatus::Running,
        })
        .child(inner)
}

fn code_section(label: &'static str, body: &str, t: &Theme) -> El {
    col()
        .key(label)
        .gap(3.0)
        .w_full()
        .child(ui::section(label, t))
        .child(ui::code(body, t))
}

fn diff_sections(tool: &Tool, t: &Theme) -> El {
    col()
        .role("group")
        .label("Changes")
        .w_full()
        .gap(8.0)
        .children(tool.diffs.iter().enumerate().map(|(index, diff)| {
            let file = FileDiff::between(
                diff.old.as_ref().map(|_| diff.path.clone()),
                Some(diff.path.clone()),
                diff.old.as_deref().unwrap_or_default(),
                &diff.new,
            );
            let mut section = col().key(format!("diff-{index}")).w_full().gap(4.0).child(
                ui::section(file.path(), t)
                    .truncate()
                    .help_when_truncated(file.path()),
            );
            if file.hunks.is_empty() {
                section = section.child(ui::section(
                    if file.binary {
                        "Binary change"
                    } else {
                        "No text changes"
                    },
                    t,
                ));
            }
            section.children(file.hunks.iter().enumerate().map(|(i, hunk)| {
                let mut body = String::new();
                let mut spans = Vec::new();
                for line in &hunk.lines {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    let start = body.len();
                    let (prefix, color) = match line.kind {
                        LineKind::Added => ('+', t.success),
                        LineKind::Removed => ('-', t.danger),
                        LineKind::Context => (' ', t.muted),
                    };
                    body.push(prefix);
                    body.push(' ');
                    body.push_str(&line.text);
                    spans.push(Span {
                        color: Some(color),
                        ..Span::new(start, body.len())
                    });
                }
                col()
                    .key(format!("hunk-{i}"))
                    .w_full()
                    .gap(1.0)
                    .child(text(hunk.header()).font(t.mono_small).color(t.faint))
                    .child(
                        rich(body, spans)
                            .font(t.mono)
                            .wrap_anywhere()
                            .w_full()
                            .selectable(true)
                            .selection_color(t.k.selection()),
                    )
            }))
        }))
}

fn plan(key: &str, steps: &[PlanStep], t: &Theme) -> El {
    col()
        .key(key)
        .role("list")
        .label("Plan")
        .w_full()
        .padding(10.0)
        .gap(5.0)
        .radius(t.k.radius.md)
        .border(1.0, t.hairline)
        .child(ui::section("Plan", t))
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, label) = match step.status {
                ToolStatus::Done => (icon(t.k.glyph.check_small, 13.0).color(t.success), "Done"),
                ToolStatus::Running => (ui::spinner(12.0, t), "In progress"),
                _ => (
                    el().size(10.0, 10.0).radius(3.0).border(1.0, t.faint),
                    "Pending",
                ),
            };
            row()
                .key(format!("step-{index}"))
                .role("listitem")
                .description(label)
                .gap(8.0)
                .center_y()
                .child(el().size(14.0, 16.0).center().child(mark).a11y_hidden())
                .child(
                    text(&step.text)
                        .font(t.ui)
                        .color(if step.status == ToolStatus::Pending {
                            t.muted
                        } else {
                            t.text
                        })
                        .grow(1.0)
                        .shrink()
                        .wrap(),
                )
        }))
}

/// A subagent call: "↳ title", opening the child conversation.
fn subagent_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let title = tool
        .child
        .and_then(|child| s.summary(child))
        .and_then(|summary| summary.title.as_ref())
        .unwrap_or(&tool.summary)
        .clone();
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
        .child(icon(&theme::BRANCH, 13.0).color(if failed { t.danger } else { t.muted }))
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
        .child(el().size(12.0, 20.0).center().child(ui::dot(t.danger, 6.0)))
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
