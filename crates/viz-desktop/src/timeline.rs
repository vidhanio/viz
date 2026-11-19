//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.
//!
//! Every activity row (tools, reasoning, progress, the working line) shares
//! one layout: a glyph at the card inset, its label one icon step in, so
//! activity lines up with the text inside cards above and below it.

use rune::{El, LoopSpec, ScrollSpec, Span, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    ButtonVariant, Markdown,
    diff::{FileDiff, LineKind},
};
use viz_client::protocol::ConversationStatus;

use crate::{
    app::{Desktop, Status},
    attachments, changes, sidebar,
    theme::{
        self, CARD_INSET, EMPTY_TOP, HAIRLINE, ICON, ICON_LARGE, ICON_SMALL, S2, S4, S8, S12, S16,
        S24, S32, S40, Size, Theme,
    },
    thread::{Item, PlanStep, Tool, ToolStatus, User},
    ui::{self, Card},
};

/// The tallest an expanded tool block gets before it scrolls.
const BLOCK_HEIGHT: f32 = 280.0;
/// An activity row's height.
const ACTIVITY_ROW: f32 = Size::Default.height();
/// Where an activity row's label starts: the card inset, a glyph and a gap.
const ACTIVITY_TEXT: f32 = CARD_INSET + ICON + S8;
/// The empty state's shortcut labels' column.
const SHORTCUT_LABEL: f32 = S32 + S32 + S8;
/// The longest a subagent's preview line gets.
const PREVIEW_CHARS: usize = 280;

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
        (Kind::Turn, _) | (_, Kind::Turn) | (Kind::Activity, Kind::Activity) => 0.0,
        (Kind::Mine, _) => S24,
        (_, Kind::Mine) => S32,
        _ => S16,
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
    let empty = thread.items.is_empty()
        && thread.before.is_none()
        && !s.busy()
        && !thread.live.busy
        && thread.live.failure.is_none();
    let body = if empty {
        empty_state(s, t).pt(EMPTY_TOP)
    } else {
        let mut rows = Rows {
            rows: Vec::new(),
            last: None,
        };
        if let Some(divider) = parent_divider(s, t) {
            rows.push(Kind::Notice, divider);
        }
        if thread.before.is_some() {
            rows.push(Kind::Notice, load_earlier(s, t));
        }
        items(s, &thread.items, false, &mut rows, t);
        live(s, &mut rows, t);
        col().key("items").children(rows.rows)
    };
    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .child(
            ui::reading(body)
                .key("timeline-body")
                .min_h(0.0)
                .pt(S24)
                .pb(S24),
        )
}

/// Older history, between two rules.
fn load_earlier(s: &Desktop, t: &Theme) -> El {
    let label = if s.history_loading {
        "Loading earlier"
    } else {
        "Load earlier"
    };
    row()
        .key("load-earlier")
        .w_full()
        .gap(S8)
        .center_y()
        .child(ui::rule(t).grow(1.0).w(0.0))
        .child(
            ui::button(label, ButtonVariant::GhostMuted, Size::Small)
                .disabled(s.history_loading || !s.connected())
                .on_press_tx(Desktop::load_earlier)
                .build(&t.k),
        )
        .child(ui::rule(t).grow(1.0).w(0.0))
}

/// A subagent's first row: whose it is, and the way back.
fn parent_divider(s: &Desktop, t: &Theme) -> Option<El> {
    let parent = s
        .selected
        .and_then(|id| s.summary(id))
        .and_then(|chat| chat.parent)
        .filter(|parent| s.summary(*parent).is_some())?;
    let title = s.title(parent).to_owned();
    Some(
        row()
            .key("parent-divider")
            .role("separator")
            .label(format!("Subagent of {title}"))
            .w_full()
            .gap(S8)
            .center_y()
            .child(ui::rule(t).grow(1.0).w(0.0))
            .child(icon(&ui::BOT, ICON_SMALL).color(t.faint))
            .child(text("Subagent of").font(t.small).color(t.faint))
            .child(
                text(title)
                    .font(t.small_medium)
                    .color(t.muted)
                    .truncate()
                    .min_w(0.0)
                    .shrink(),
            )
            .child(
                ui::button("Open parent", ButtonVariant::GhostMuted, Size::Small)
                    .icon(&ui::CORNER_LEFT_UP)
                    .on_press_tx(move |s: &mut Desktop, tx| s.select(parent, tx))
                    .build(&t.k)
                    .key("open-parent"),
            )
            .child(ui::rule(t).grow(1.0).w(0.0)),
    )
}

/// `item`, when it spawned a subagent.
fn spawned(item: &Item) -> Option<&Tool> {
    match item {
        Item::Tool(tool) if subagent_call(tool) == Some(Call::Spawn) => Some(tool),
        _ => None,
    }
}

/// Builds `items` into `rows`. `streaming` marks the in-flight answer.
fn items(s: &Desktop, items: &[Item], streaming: bool, rows: &mut Rows, t: &Theme) {
    let mut index = 0;
    while let Some(item) = items.get(index) {
        index += 1;
        match item {
            Item::User(user) => rows.push(Kind::Mine, mine(s, user, t)),
            Item::Text { key, text } => rows.push(Kind::Prose, prose(key, text, streaming, t)),
            Item::Thinking {
                key,
                text,
                duration_ms,
            } => rows.push(
                Kind::Activity,
                thinking(s, key, text, *duration_ms, streaming, t),
            ),
            Item::Tool(tool) => match subagent_call(tool) {
                Some(Call::Spawn) => {
                    // Subagents started back to back gather into one group.
                    let mut run = vec![tool];
                    while let Some(next) = items.get(index).and_then(spawned) {
                        run.push(next);
                        index += 1;
                    }
                    let row = if run.len() == 1 {
                        subagent_link(s, tool, t)
                    } else {
                        subagent_group(s, &run, t)
                    };
                    rows.push(Kind::Activity, row);
                }
                Some(call) => rows.push(Kind::Activity, subagent_row(s, tool, call, t)),
                None => rows.push(Kind::Activity, tool_row(s, tool, t)),
            },
            Item::Plan { key, steps } => rows.push(Kind::Prose, plan(key, steps, t)),
            Item::Progress { key, text: words } => rows.push(
                Kind::Activity,
                text(words)
                    .key(key)
                    .font(t.ui)
                    .color(t.muted)
                    .wrap()
                    .w_full()
                    .pl(ACTIVITY_TEXT)
                    .pr(CARD_INSET),
            ),
            Item::Divider { key, label } => rows.push(Kind::Turn, divider(key, *label, t)),
            Item::Notice { key, text } => rows.push(Kind::Notice, notice(key, text, t)),
        }
    }
}

/// An activity row: a glyph at the card inset, then the label.
fn activity(key: &str, mark: El) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(ACTIVITY_ROW)
        .px(CARD_INSET)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(ui::lead(mark, ACTIVITY_ROW))
}

/// An activity row that opens and closes on press.
fn disclosure(key: &str, open: bool, t: &Theme) -> El {
    let toggle = key.to_owned();
    activity(
        "header",
        icon(
            if open {
                &ui::CHEVRON_OPEN
            } else {
                &ui::CHEVRON
            },
            ICON_SMALL,
        )
        .color(t.faint),
    )
    .role("button")
    .expanded(open)
    .focusable(true)
    .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
    .hover(t.hover)
    .on_press(move |s: &mut Desktop| s.toggle(&toggle))
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
        let pending_agents = s.selected.map_or(0, |id| {
            sidebar::subagents(&s.conversations, id)
                .iter()
                .filter(|chat| chat.status == ConversationStatus::Working)
                .count()
        });
        let status = match (&live.retry, live.compacting) {
            (Some(error), _) => Some((format!("Retrying: {error}"), t.warning)),
            (None, true) => Some(("Compacting context".to_owned(), t.muted)),
            (None, false) if pending_agents > 0 && !writing => Some((
                if pending_agents == 1 {
                    "Waiting on a subagent".to_owned()
                } else {
                    format!("Waiting on {pending_agents} subagents")
                },
                t.muted,
            )),
            (None, false) if live.partial.is_empty() && !running => {
                Some(("Working".to_owned(), t.muted))
            }
            (None, false) => None,
        };
        if let Some((label, color)) = status {
            rows.push(
                Kind::Activity,
                activity("working", ui::spinner(t))
                    .role("status")
                    .label(label.clone())
                    .child(text(label).font(t.ui).color(color).truncate().shrink()),
            );
        } else if writing {
            rows.push(
                Kind::Activity,
                el().key("caret")
                    .w(S2)
                    .h(ICON)
                    .radius(HAIRLINE)
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
    match s.status {
        Status::Error(_) => {
            return ui::empty(
                None,
                "Offline",
                "Start viz-backend to reconnect.",
                Some(
                    ui::button("Retry", ButtonVariant::Outline, Size::Default)
                        .on_press_tx(Desktop::reconnect)
                        .build(&t.k)
                        .key("retry"),
                ),
                t,
            );
        }
        Status::Connecting => return ui::empty(None, "Connecting", "", None, t),
        Status::Connected { .. } => {}
    }
    if s.selected.is_none() && s.new_chat.is_none() {
        return ui::empty(None, "No chat selected", "", None, t);
    }
    // First run: nothing to chat in yet, so the one action is adding a
    // project; the orchestrator's intro carries it when that's what's open.
    let onboarding = s.projects.is_empty() && s.new_chat.is_none();
    let add_project = || {
        ui::button("Add project", ButtonVariant::Submit, Size::Large)
            .icon(&ui::PLUS)
            .on_press_tx(sidebar::open_add_project)
            .build(&t.k)
            .key("add-project-empty")
    };
    if s.selected.is_some() && s.selected == s.root {
        let intro = ui::empty(
            Some(icon(&ui::STAR, ICON_LARGE).color(t.accent)),
            "Orchestrator",
            "Your main agent. It keeps memory across chats, answers your \
             connectors, and starts chats when work needs its own thread.",
            onboarding.then(add_project),
            t,
        );
        return if onboarding {
            intro
        } else {
            intro.child(shortcuts(&[("K", "Commands"), ("N", "New chat")], t))
        };
    }
    if onboarding {
        return ui::empty(
            None,
            "Add a project",
            "Chats run in a project’s folder, on its branch.",
            Some(add_project()),
            t,
        );
    }
    chat_hint(s, t)
}

/// An empty chat: where it runs, and the few keys worth knowing.
fn chat_hint(s: &Desktop, t: &Theme) -> El {
    let title = s
        .project()
        .map_or("New chat", |project| project.name.as_str());
    let status = s.new_chat.as_ref().and_then(|chat| {
        if chat.request.is_some() {
            Some("Opening session…")
        } else {
            (s.harnesses.len() > 1).then_some("Choose a harness below")
        }
    });
    let mut hint = ui::empty(None, title, status.unwrap_or_default(), None, t);
    if status.is_none()
        && let Some(branch) = s.branch()
    {
        hint = hint.child(
            row()
                .key("empty-branch")
                .gap(S4)
                .center_y()
                .child(icon(&ui::GIT_BRANCH, ICON_SMALL).color(t.muted))
                .child(text(branch).font(t.ui).color(t.muted)),
        );
    }
    hint.child(shortcuts(
        &[("N", "New chat"), ("K", "Commands"), ("D", "Changes")],
        t,
    ))
}

fn shortcuts(keys: &[(&str, &str)], t: &Theme) -> El {
    col()
        .key("shortcuts")
        .mt(S16)
        .gap(S8)
        .children(keys.iter().map(|(key, label)| {
            row()
                .gap(S12)
                .center_y()
                .child(
                    text(*label)
                        .font(t.small)
                        .color(t.faint)
                        .w(SHORTCUT_LABEL)
                        .align(rune::Align::End),
                )
                .child(ui::kbd(key, t))
        }))
}

/// A user message sits on a neutral surface.
fn mine(s: &Desktop, user: &User, t: &Theme) -> El {
    let mut message = Card::Message
        .build(t)
        .key(&user.key)
        .role("article")
        .label("You")
        .hover_group_focus();
    if let Some(turn) = user.turn
        && s.conversation
            .as_ref()
            .is_some_and(|state| state.git.is_some())
    {
        let action = |label: &str| ui::button(label, ButtonVariant::GhostMuted, Size::Small);
        message = message.child(
            row()
                .key("prompt-actions")
                .w_full()
                .h(Size::Small.height())
                .gap(S4)
                .center_y()
                .child(ui::section("You", t).grow(1.0))
                .child(
                    row()
                        .key("actions")
                        .gap(S2)
                        .group_opacity(0.0, 1.0)
                        .child(
                            action("Changes")
                                .disabled(!s.connected())
                                .on_press_tx(move |s: &mut Desktop, tx| changes::turn(s, tx, turn))
                                .build(&t.k)
                                .key("turn-changes"),
                        )
                        .child(
                            action("Revert to here")
                                .disabled(!s.connected() || s.busy())
                                .on_press_tx(move |s: &mut Desktop, tx| {
                                    changes::confirm_revert(s, tx, turn);
                                })
                                .build(&t.k)
                                .key("turn-revert"),
                        ),
                ),
        );
    }
    if !user.text.is_empty() {
        message = message.child(
            text(&user.text)
                .key("text")
                .font(t.body)
                .color(t.text)
                .wrap()
                .w_full()
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    }
    if !user.attachments.is_empty() {
        message = message.child(
            row()
                .key("message-attachments")
                .w_full()
                .gap(S4)
                .flex_wrap()
                .children(user.attachments.iter().enumerate().map(|(index, name)| {
                    attachments::chip(name, t).key(format!("attachment-{index}"))
                })),
        );
    }
    message
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

/// Reasoning: one faint row that opens into the thought.
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
    let header = disclosure(key, open, t)
        .label(title.clone())
        .child(text(title).font(t.ui).color(t.muted).truncate().shrink());
    let mut el = col().key(key.to_owned()).w_full().child(header);
    if open {
        el = el.child(
            text(body)
                .key("thought")
                .w_full()
                .pl(ACTIVITY_TEXT)
                .pr(CARD_INSET)
                .pb(S8)
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    }
    el
}

/// What a settled or running call says at its right end.
fn meta(tool: &Tool, t: &Theme) -> Option<El> {
    let faint = |words: String| text(words).font(t.mono_small).color(t.faint);
    match tool.status {
        ToolStatus::Running => Some(ui::spinner(t)),
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
    let failed = tool.status == ToolStatus::Failed;
    let mut header = disclosure(&tool.key, open, t)
        .label(format!("{} {}", tool.name, tool.summary))
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
    let mut el = col().key(tool.key.clone()).w_full().child(header);
    if open {
        el = el.child(output(tool, t));
    }
    el
}

/// An open tool row's arguments and output, in a scrolling mono card.
fn output(tool: &Tool, t: &Theme) -> El {
    let mut inner = col().key("inner").w_full().gap(S12).padding(CARD_INSET);
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
    Card::Surface
        .frame(t)
        .key("body")
        .mt(S4)
        .mb(S8)
        .max_h(BLOCK_HEIGHT)
        .min_h(0.0)
        .clip_rounded(theme::RADIUS_CARD)
        .scroll(ScrollSpec {
            stick_bottom: tool.status == ToolStatus::Running,
        })
        .child(inner)
}

fn code_section(label: &'static str, body: &str, t: &Theme) -> El {
    col()
        .key(label)
        .gap(S4)
        .w_full()
        .child(ui::section(label, t))
        .child(ui::code(body, t))
}

fn diff_sections(tool: &Tool, t: &Theme) -> El {
    col()
        .role("group")
        .label("Changes")
        .w_full()
        .gap(S8)
        .children(tool.diffs.iter().enumerate().map(|(index, diff)| {
            let file = FileDiff::between(
                diff.old.as_ref().map(|_| diff.path.clone()),
                Some(diff.path.clone()),
                diff.old.as_deref().unwrap_or_default(),
                &diff.new,
            );
            let mut section = col().key(format!("diff-{index}")).w_full().gap(S4).child(
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
                    .gap(S2)
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
    let line = t.ui.line_height;
    Card::Outline
        .build(t)
        .key(key)
        .role("list")
        .label("Plan")
        .gap(S4)
        .child(ui::section("Plan", t))
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, label) = match step.status {
                ToolStatus::Done => (icon(&ui::CHECK, ICON).color(t.success), "Done"),
                ToolStatus::Running => (ui::spinner(t), "In progress"),
                _ => (
                    el().size(ICON_SMALL - S2, ICON_SMALL - S2)
                        .radius(theme::S2 + HAIRLINE)
                        .border(HAIRLINE, t.faint),
                    "Pending",
                ),
            };
            row()
                .key(format!("step-{index}"))
                .role("listitem")
                .description(label)
                .w_full()
                .gap(S8)
                .items_start()
                .child(ui::lead(mark, line))
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

/// How a call relates to a subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Spawn,
    Message,
    Read,
    List,
    Cancel,
}

fn subagent_call(tool: &Tool) -> Option<Call> {
    let name = tool.viz.as_deref().unwrap_or(tool.name.as_str());
    match name {
        "subagent" | "spawn_subagent" => Some(Call::Spawn),
        "message_subagent" => Some(Call::Message),
        "read_subagent" => Some(Call::Read),
        "list_subagents" => Some(Call::List),
        "cancel_subagent" => Some(Call::Cancel),
        _ => tool.child.map(|_| Call::Spawn),
    }
}

/// A subagent's title: its conversation's, or the call's summary.
fn subagent_title<'a>(s: &'a Desktop, tool: &'a Tool) -> &'a str {
    tool.child
        .and_then(|child| s.summary(child))
        .map(|summary| summary.title.as_str())
        .filter(|title| !title.is_empty())
        .unwrap_or(&tool.summary)
}

/// Where a subagent stands: its conversation's live status, else its call's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Working,
    Done,
    Failed,
    Stopped,
}

impl Agent {
    fn of(s: &Desktop, tool: &Tool) -> Self {
        let status = tool
            .child
            .and_then(|child| s.summary(child))
            .map(|chat| chat.status);
        match (status, tool.status) {
            (Some(ConversationStatus::Working), _)
            | (None, ToolStatus::Running | ToolStatus::Pending) => Self::Working,
            (Some(ConversationStatus::Failed), _) | (None, ToolStatus::Failed) => Self::Failed,
            (None, ToolStatus::Abandoned) => Self::Stopped,
            _ => Self::Done,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    fn mark(self, t: &Theme) -> El {
        match self {
            Self::Working => ui::spinner(t),
            Self::Done => ui::dot(t.success),
            Self::Failed => ui::dot(t.danger),
            Self::Stopped => ui::dot(t.faint),
        }
    }
}

/// A subagent's preview line: its latest progress while it works, its result
/// after, as one line of plain text, clamped.
fn preview(tool: &Tool) -> Option<String> {
    let line = tool
        .output
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '-', '*', '>', ' ']))
        .find(|line| !line.is_empty())?;
    let mut plain: String = line.chars().filter(|ch| *ch != '`').collect();
    if let Some((cut, _)) = plain.char_indices().nth(PREVIEW_CHARS) {
        plain.truncate(cut);
        plain.push('…');
    }
    Some(plain)
}

/// A spawned subagent as a link: its state, title and preview line, when it
/// last moved, and a way into its conversation.
fn subagent_link(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let title = subagent_title(s, tool).to_owned();
    let agent = Agent::of(s, tool);
    let when = tool.duration_ms.map(theme::duration).or_else(|| {
        tool.child
            .and_then(|child| s.summary(child))
            .and_then(|chat| theme::ago(chat.updated_at))
    });
    let mut labels = col().grow(1.0).shrink().min_w(0.0).child(
        text(title.clone())
            .key("title")
            .font(t.ui_medium)
            .color(t.text)
            .truncate()
            .help_when_truncated(title.clone()),
    );
    if let Some(preview) = preview(tool) {
        labels = labels.child(
            text(preview)
                .key("preview")
                .font(t.small)
                .color(t.muted)
                .truncate(),
        );
    }
    let mut link = row()
        .key(tool.key.clone())
        .role("link")
        .label(format!("Subagent: {title}"))
        .description(agent.word())
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(ui::lead(agent.mark(t), t.ui.line_height))
        .child(labels)
        .children(when.map(|when| text(when).font(t.mono_small).color(t.faint)));
    if let Some(child) = tool.child {
        link = link
            .hover(t.hover)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| s.select(child, tx))
            .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint));
    }
    link
}

/// Two or more subagents one turn started together: a count and how they
/// stand, opening into their links.
fn subagent_group(s: &Desktop, tools: &[&Tool], t: &Theme) -> El {
    let key = format!("{}-group", tools[0].key);
    let open = s.expanded.contains(&key);
    let agents: Vec<_> = tools.iter().map(|tool| Agent::of(s, tool)).collect();
    let summary = [Agent::Working, Agent::Done, Agent::Failed, Agent::Stopped]
        .into_iter()
        .filter_map(|state| {
            let count = agents.iter().filter(|agent| **agent == state).count();
            (count > 0).then(|| format!("{count} {}", state.word()))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let label = format!("{} subagents", tools.len());
    let header = disclosure(&key, open, t)
        .key("group")
        .label(label.clone())
        .description(summary.clone())
        .child(text(label).font(t.ui_medium).color(t.text))
        .child(
            text(summary)
                .font(t.small)
                .color(if agents.contains(&Agent::Failed) {
                    t.danger
                } else {
                    t.muted
                })
                .truncate()
                .grow(1.0)
                .shrink(),
        );
    let mut group = col().key(key).w_full().child(header);
    if open {
        group = group.child(
            Card::Outline
                .frame(t)
                .key("members")
                .mt(S4)
                .padding(S4)
                .children(tools.iter().map(|tool| subagent_link(s, tool, t))),
        );
    }
    group
}

/// Talking to, reading, listing or stopping a subagent: one activity row
/// that opens the subagent it names.
fn subagent_row(s: &Desktop, tool: &Tool, call: Call, t: &Theme) -> El {
    let verb = match call {
        Call::Spawn | Call::Message => "Messaged",
        Call::Read => "Read",
        Call::List => "Listed subagents",
        Call::Cancel => "Stopped",
    };
    let name = (call != Call::List).then(|| subagent_title(s, tool).to_owned());
    let failed = tool.status == ToolStatus::Failed;
    let label = name
        .as_ref()
        .map_or_else(|| verb.to_owned(), |name| format!("{verb} {name}"));
    let mut line = activity(
        &tool.key,
        icon(&ui::BOT, ICON_SMALL).color(if failed { t.danger } else { t.faint }),
    )
    .role("link")
    .label(label)
    .child(text(verb).font(t.ui).color(t.muted))
    .children(name.map(|name| {
        text(name)
            .key("title")
            .font(t.ui)
            .color(t.text)
            .truncate()
            .grow(1.0)
            .shrink()
    }));
    if call == Call::List {
        line = line.child(el().grow(1.0).a11y_hidden());
    }
    if let Some(meta) = meta(tool, t) {
        line = line.child(meta.key("meta"));
    }
    match tool.child {
        Some(child) => line
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .help("Open the subagent")
            .hover(t.hover)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| s.select(child, tx)),
        None => line,
    }
}

/// A turn boundary: room, and a short faint rule; or a quiet label for
/// other markers.
fn divider(key: &str, label: Option<&str>, t: &Theme) -> El {
    let mark = label.map_or_else(
        || el().w(S40).h(HAIRLINE).fill(t.hairline),
        |label| text(label.to_owned()).font(t.small).color(t.faint),
    );
    row()
        .key(key.to_owned())
        .role("separator")
        .label(label.unwrap_or("New turn").to_owned())
        .w_full()
        .py(S24)
        .center()
        .child(mark)
}

/// An answer that didn't come: a danger-tinted line.
fn notice(key: &str, message: &str, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .role("alert")
        .w_full()
        .px(CARD_INSET)
        .gap(S8)
        .items_start()
        .child(ui::lead(ui::dot(t.danger), t.ui.line_height))
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
