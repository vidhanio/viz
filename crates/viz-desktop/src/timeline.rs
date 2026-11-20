//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.
//!
//! Every activity row (tools, reasoning, progress, the working line) shares
//! one layout: a glyph at the card inset, its label one icon step in, so
//! activity lines up with the text inside cards above and below it.

use rune::{El, LoopSpec, ScrollSpec, Span, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Markdown, Menu, MenuItem, Placement,
    diff::{FileDiff, LineKind},
};
use viz_client::protocol::ConversationStatus;

use crate::{
    app::{Desktop, NewChat, Status},
    attachments, sidebar,
    theme::{
        self, CARD_INSET, EMPTY_TOP, EMPTY_WIDTH, HAIRLINE, ICON, ICON_LARGE, ICON_SMALL, S2, S4,
        S8, S12, S16, S24, S32, S40, Size, Theme,
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
/// The widest a user bubble gets, as a share of the reading column.
const BUBBLE_FRAC: f32 = 0.8;

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
pub fn timeline(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let thread = &s.thread;
    let empty = thread.items.is_empty()
        && thread.before.is_none()
        && !s.busy()
        && !thread.live.busy
        && thread.live.failure.is_none();
    let body = if empty {
        empty_state(s, t, layer).pt(EMPTY_TOP)
    } else {
        let mut rows = Rows {
            rows: Vec::new(),
            last: None,
        };
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
        "loading earlier"
    } else {
        "load earlier"
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
            (Some(error), _) => Some((format!("retrying: {error}"), t.warning)),
            (None, true) => Some(("compacting context".to_owned(), t.muted)),
            (None, false) if pending_agents > 0 && !writing => Some((
                if pending_agents == 1 {
                    "waiting on a subagent".to_owned()
                } else {
                    format!("waiting on {pending_agents} subagents")
                },
                t.muted,
            )),
            (None, false) if live.partial.is_empty() && !running => {
                Some(("working".to_owned(), t.muted))
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
fn empty_state(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    match s.status {
        Status::Error(_) => {
            return ui::empty(
                None,
                "offline",
                "start viz-backend to reconnect.",
                Some(
                    ui::button("retry", ButtonVariant::Outline, Size::Default)
                        .on_press_tx(Desktop::reconnect)
                        .build(&t.k)
                        .key("retry"),
                ),
                t,
            );
        }
        Status::Connecting => return ui::empty(None, "connecting", "", None, t),
        Status::Connected { .. } => {}
    }
    if s.selected.is_none() && s.new_chat.is_none() {
        return ui::empty(None, "no chat selected", "", None, t);
    }
    // First run: nothing to chat in yet, so the one action is adding a
    // project; the orchestrator's intro carries it when that's what's open.
    let onboarding = s.projects.is_empty() && s.new_chat.is_none();
    let add_project = || {
        ui::button("add project", ButtonVariant::Submit, Size::Large)
            .icon(&ui::PLUS)
            .on_press_tx(sidebar::open_add_project)
            .build(&t.k)
            .key("add-project-empty")
    };
    if s.orchestrator_open() {
        let intro = ui::empty(
            Some(icon(&ui::STAR, ICON_LARGE).color(t.accent)),
            "orchestrator",
            "remembers across chats and starts agents for you.",
            onboarding.then(add_project),
            t,
        );
        return if onboarding {
            intro
        } else {
            intro.child(shortcuts(&[("K", "commands"), ("N", "new chat")], t))
        };
    }
    if onboarding {
        return ui::empty(
            None,
            "add a project",
            "chats run in a project’s folder.",
            Some(add_project()),
            t,
        );
    }
    chat_hint(s, t, layer)
}

/// An empty chat: where it runs, and the few keys that work from here. A
/// draft's place is still a choice, so it shows the project picker instead.
fn chat_hint(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let Some(chat) = &s.new_chat else {
        let title = s
            .project()
            .map_or("new chat", |project| project.name.as_str());
        return ui::empty(None, title, "", None, t)
            .child(shortcuts(&[("N", "new chat"), ("K", "commands")], t));
    };
    let choose_harness = !chat.sending
        && chat.harness.is_none()
        && s.harnesses.len() > 1
        && s.harnesses.iter().all(|harness| !harness.default);
    // `ui::empty`'s column, with the picker row where its title goes.
    col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S8)
        .items_center()
        .child(draft_project(s, chat, t, layer))
        .children(choose_harness.then(|| {
            text("choose a harness below")
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center)
        }))
        // A draft is already the new chat.
        .child(shortcuts(&[("K", "commands")], t))
}

/// The `open_picker` value of a draft's project picker.
pub const DRAFT_PROJECT: &str = "draft-project";

/// Where a draft will run: a picker of every project and none, and an add
/// button whose new project the draft takes once the backend has it
/// (`Desktop::project_added`). Fixed once the first message is on its way.
fn draft_project(s: &Desktop, chat: &NewChat, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = s.open_picker.as_deref() == Some(DRAFT_PROJECT) && !chat.sending;
    let current = s
        .project()
        .map_or("no project", |project| project.name.as_str());
    let trigger = ui::select_trigger(
        DRAFT_PROJECT,
        "project",
        Some(current),
        open,
        ui::MODEL_TRIGGER_MAX,
        t,
    )
    .on_press(|s: &mut Desktop| {
        s.open_picker = if s.open_picker.as_deref() == Some(DRAFT_PROJECT)
            || s.new_chat.as_ref().is_none_or(|chat| chat.sending)
        {
            None
        } else {
            Some(DRAFT_PROJECT.to_owned())
        };
    });
    let mut menu = Menu::new(format!("{DRAFT_PROJECT}-picker"))
        .standard()
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.open_picker = None)
        .trigger(trigger);
    if open {
        let choice = |label: &str, glyph: &'static rune::Icon, id: Option<&str>| {
            let picked = chat.project.as_deref() == id;
            let id = id.map(str::to_owned);
            MenuItem::new(label)
                .icon(glyph)
                .checked(picked)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.open_picker = None;
                    tx.focus("draft", false);
                    if let Some(chat) = &mut s.new_chat {
                        chat.project.clone_from(&id);
                    }
                })
        };
        menu = menu.item(choice("no project", &ui::CHAT, None)).separator();
        menu = ui::menu_items(
            menu,
            s.projects
                .iter()
                .map(|project| choice(&project.name, &ui::FOLDER, Some(&project.id))),
        );
    }
    let add = ui::icon_button(
        &ui::FOLDER_PLUS,
        "add project",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(chat.sending)
    .on_press_tx(sidebar::open_add_project)
    .build(&t.k)
    .key("draft-add-project");
    row()
        .key("draft-place")
        .gap(S4)
        .center_y()
        .min_w(0.0)
        .child(menu.build(&t.overlay, layer).min_w(0.0).shrink())
        .child(add)
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

/// A user message: a bubble on a neutral surface at the right of the column.
/// It hugs short text and wraps long text at [`BUBBLE_FRAC`] of the column.
fn mine(user: &User, t: &Theme) -> El {
    let mut message = Card::Message
        .build(t)
        .key(&user.key)
        .role("article")
        .label("you");
    if !user.text.is_empty() {
        message = message.child(
            text(&user.text)
                .key("text")
                .font(t.body)
                .color(t.text)
                .wrap()
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    }
    if !user.attachments.is_empty() {
        message = message.child(
            row()
                .key("message-attachments")
                .self_end()
                .gap(S4)
                .flex_wrap()
                .justify_end()
                .children(user.attachments.iter().enumerate().map(|(index, name)| {
                    attachments::chip(name, t).key(format!("attachment-{index}"))
                })),
        );
    }
    // The spacer holds the column's other share (it never shrinks); the
    // bubble's wrapper shrinks into the rest, so the card's own `w_full`
    // fills only its content's width.
    row()
        .key(format!("{}-row", user.key))
        .w_full()
        .justify_end()
        .child(el().w_frac(1.0 - BUBBLE_FRAC))
        .child(col().min_w(0.0).shrink().child(message))
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
        _ if streaming => "thinking…".to_owned(),
        Some(ms) => format!("thought for {}s", (ms + 500) / 1000),
        None => "thought".to_owned(),
    };
    let body = if thought.trim().is_empty() {
        "reasoning hidden"
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
        inner = inner.child(code_section("input", &tool.arguments, t));
    }
    if !tool.output.is_empty() {
        inner = inner.child(code_section("output", &tool.output, t));
    }
    if !tool.locations.is_empty() {
        inner = inner.child(code_section("paths", &tool.locations.join("\n"), t));
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
        inner = inner.child(ui::section("no details", t));
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
        .label("changes")
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
                        "binary change"
                    } else {
                        "no text changes"
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
        .label("plan")
        .gap(S4)
        .child(ui::section("plan", t))
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, label) = match step.status {
                ToolStatus::Done => (icon(&ui::CHECK, ICON).color(t.success), "done"),
                ToolStatus::Running => (ui::spinner(t), "in progress"),
                _ => (
                    el().size(ICON_SMALL - S2, ICON_SMALL - S2)
                        .radius(theme::S2 + HAIRLINE)
                        .border(HAIRLINE, t.faint),
                    "pending",
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
        "spawn_subagent" => Some(Call::Spawn),
        "spawn_chat" if tool.child.is_some() => Some(Call::Spawn),
        "message_subagent" => Some(Call::Message),
        "read_subagent" => Some(Call::Read),
        "list_subagents" => Some(Call::List),
        "cancel_subagent" => Some(Call::Cancel),
        _ => None,
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
        .label(format!("subagent: {title}"))
        .description(agent.word())
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(ui::lead(agent.mark(t), t.ui.line_height))
        .child(labels)
        .children(when.map(|when| text(when).font(t.small).color(t.faint)));
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
        Call::Spawn | Call::Message => "messaged",
        Call::Read => "read",
        Call::List => "listed subagents",
        Call::Cancel => "stopped",
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
            .help("open the subagent")
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
        .label(label.unwrap_or("new turn").to_owned())
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
