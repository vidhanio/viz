//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.
//!
//! Every activity row (tools, reasoning, progress, the working line) shares
//! one layout: a glyph at the card inset, its label one icon step in, so
//! activity lines up with the text inside cards above and below it.

use rune::{El, LoopSpec, ScrollSpec, Span, Transition, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Markdown, Menu, MenuItem, Placement, ProgressCircle,
    diff::{FileDiff, LineKind},
};
use viz_client::protocol::ThreadStatus;

use crate::{
    app::{Desktop, NewThread, Status},
    attachments, header, sidebar,
    theme::{
        self, CARD_INSET, EMPTY_TOP, EMPTY_WIDTH, HAIRLINE, ICON, ICON_SMALL, S2, S4, S8, S12, S16,
        S24, S32, S40, Size, Theme,
    },
    thread::{Item, PlanStep, Tool, ToolStatus, User, is_command},
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
/// The widest a user bubble gets, as a share of the reading column.
const BUBBLE_FRAC: f32 = 0.8;

/// What a row is, for the space above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mine,
    Prose,
    Activity,
    Notice,
}

/// Points above a row of kind `next` that follows one of kind `prev`.
const fn space(prev: Kind, next: Kind) -> f32 {
    match (prev, next) {
        (Kind::Activity, Kind::Activity) => 0.0,
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
    let empty = thread.items.is_empty() && thread.before.is_none() && !s.busy();
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
        items(
            s,
            thread
                .displayed_history()
                .iter()
                .map(|item| (item, false))
                .chain(thread.acp_partial.iter().map(|item| (item, true))),
            &mut rows,
            t,
        );
        live(s, &mut rows, t);
        col().key("items").children(rows.rows)
    };
    let mut body = ui::reading(body)
        .key("timeline-body")
        .min_h(0.0)
        .pt(S24)
        .pb(S24);
    if !thread.items.is_empty()
        && let Some(id) = s
            .selected
            .filter(|&id| s.summary(id).is_some_and(|thread| thread.parent.is_some()))
    {
        body = body
            .key(format!("subagent-timeline-{id}"))
            .transition(Transition::asymmetric(
                Transition::opacity().combined(Transition::offset(S12, 0.0)),
                Transition::none(),
            ));
    }
    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .child(body)
}

/// Older history, between two rules.
fn load_earlier(s: &Desktop, t: &Theme) -> El {
    let label = if s.history_loading {
        "Loading earlier messages…"
    } else {
        "Load earlier messages"
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

/// Only prose, input, plans and notices interrupt an activity burst.
const fn is_activity(item: &Item) -> bool {
    matches!(item, Item::Tool(_) | Item::Thinking { .. })
}

fn items<'a>(
    s: &Desktop,
    items: impl Iterator<Item = (&'a Item, bool)> + Clone,
    rows: &mut Rows,
    t: &Theme,
) {
    let mut items = items;
    while let Some((item, streaming)) = items.next() {
        match item {
            Item::User(user) => rows.push(Kind::Mine, mine(user, t)),
            Item::Text { key, text } => rows.push(Kind::Prose, prose(key, text, streaming, t)),
            Item::Thinking { .. } | Item::Tool(_) => {
                let count = items
                    .clone()
                    .take_while(|(item, _)| is_activity(item))
                    .count();
                let burst = std::iter::once((item, streaming)).chain(items.clone().take(count));
                rows.push(Kind::Activity, activity_group(s, item.key(), &burst, t));
                for _ in 0..count {
                    items.next();
                }
            }
            Item::Plan { key, steps } => rows.push(Kind::Prose, plan(key, steps, t)),
            Item::TurnStatus { key, text: words } => rows.push(
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
            Item::Notice { key, text } => rows.push(Kind::Notice, notice(key, text, t)),
        }
    }
}

fn activity_group<'a>(
    s: &Desktop,
    first: &str,
    burst: &(impl Iterator<Item = (&'a Item, bool)> + Clone),
    t: &Theme,
) -> El {
    let key = format!("{first}-activity");
    let open = s.expanded.contains(&key);
    let mut tool_count = 0;
    let mut spawn_count = 0;
    let mut active_agents = 0;
    let mut thinking = false;
    let mut active_tool = None;
    for (item, streaming) in burst.clone() {
        match item {
            Item::Thinking { .. } => thinking |= streaming,
            Item::Tool(tool) if spawned(tool) => {
                spawn_count += 1;
                active_agents += usize::from(Agent::of(s, tool) == Agent::Working);
            }
            Item::Tool(tool) => {
                tool_count += 1;
                if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) {
                    active_tool = Some(tool);
                }
            }
            _ => {}
        }
    }
    let label = match (tool_count, spawn_count) {
        (0, 0) => if thinking { "Thinking" } else { "Thought" }.to_owned(),
        (0, n) => format!("Delegated {n} {}", plural(n, "task", "tasks")),
        (n, 0) => format!("Called {n} {}", plural(n, "tool", "tools")),
        (tools, agents) => format!(
            "Called {tools} {}, delegated {agents} {}",
            plural(tools, "tool", "tools"),
            plural(agents, "task", "tasks")
        ),
    };
    let mut heading = disclosure(&key, open, t).label(label.clone()).child(
        text(label)
            .font(t.ui_medium)
            .color(t.muted)
            .grow(1.0)
            .shrink()
            .truncate(),
    );
    if spawn_count > 0 {
        let agents = burst.clone().filter_map(|(item, _)| match item {
            Item::Tool(tool) if spawned(tool) => local_child(s, tool),
            _ => None,
        });
        heading = heading.child(agent_marks(agents, spawn_count, t));
    }
    if active_agents > 0 {
        heading = heading.child(
            text(format!("{active_agents} running"))
                .font(t.small)
                .color(t.muted),
        );
    } else if thinking {
        heading = heading.child(ui::spinner(t));
    }
    let mut group = col().key(key).w_full().child(heading);
    if open {
        let mut details = col().key("details").w_full().pl(S8).gap(S2);
        let first_spawn = burst.clone().find_map(|(item, _)| match item {
            Item::Tool(tool) if spawned(tool) => Some(tool.key.as_str()),
            _ => None,
        });
        for (item, streaming) in burst.clone() {
            match item {
                Item::Thinking { key, text } => {
                    details = details.child(thought(key, text, streaming, t));
                }
                Item::Tool(tool) if spawned(tool) => {
                    if first_spawn == Some(tool.key.as_str()) {
                        let agents = burst.clone().filter_map(|(item, _)| match item {
                            Item::Tool(tool) if spawned(tool) => Some(tool.as_ref()),
                            _ => None,
                        });
                        details = details.child(agent_stack(s, agents, spawn_count, t));
                    }
                }
                Item::Tool(tool) => {
                    details = details.child(match subagent_call(tool) {
                        Some(call) if call != Call::Spawn && tool.status != ToolStatus::Failed => {
                            subagent_row(s, tool, call, t)
                        }
                        _ => tool_row(s, tool, t),
                    });
                }
                _ => {}
            }
        }
        group = group.child(details);
    } else if let Some(tool) = active_tool {
        // Keep the current command visible even when the burst is folded.
        group = group.child(tool_row(s, tool, t).ml(S8));
    }
    group
}

const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

fn spawned(tool: &Tool) -> bool {
    tool.child.is_some()
        && subagent_call(tool) == Some(Call::Spawn)
        && tool.status != ToolStatus::Failed
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

/// The streamed answer’s caret, or a quiet line while the run has not emitted
/// activity.
fn live(s: &Desktop, rows: &mut Rows, t: &Theme) {
    if !s.busy() {
        return;
    }
    let partial = &s.thread.acp_partial;
    let running = s
        .thread
        .items
        .iter()
        .rev()
        .take_while(|item| !matches!(item, Item::User(_)))
        .any(|item| matches!(item, Item::Tool(tool) if tool.status == ToolStatus::Running));
    if partial.is_empty() && !running {
        rows.push(
            Kind::Activity,
            activity("working", ui::spinner(t))
                .role("status")
                .label("Working")
                .child(text("Working").font(t.ui).color(t.muted)),
        );
    } else if matches!(partial.last(), Some(Item::Text { .. })) {
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

/// What to show when the timeline has no messages.
fn empty_state(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    match s.status {
        Status::Error(_) => {
            return ui::empty(
                None,
                "Offline",
                "Start viz-server to reconnect.",
                Some(
                    ui::button("Retry", ButtonVariant::Outline, Size::Default)
                        .on_press_tx(Desktop::reconnect)
                        .build(&t.k)
                        .key("retry"),
                ),
                t,
            );
        }
        Status::Connecting => return ui::empty(None, "Connecting…", "", None, t),
        Status::Connected { .. } => {}
    }
    if s.selected.is_none() && s.new_thread.is_none() {
        let (title, detail, action) = if s.projects.is_empty() {
            (
                "No thread selected",
                "Threads can run in a project’s folder.",
                ui::button("Add project", ButtonVariant::Outline, Size::Large)
                    .icon(&ui::PLUS)
                    .on_press_tx(sidebar::open_add_project),
            )
        } else {
            (
                "No thread selected",
                "Select a thread or start a new one.",
                ui::button("New thread", ButtonVariant::Outline, Size::Large)
                    .icon(&ui::COMPOSE)
                    .on_press_tx(Desktop::create_here),
            )
        };
        return ui::empty(None, title, detail, Some(action.build(&t.k)), t);
    }
    thread_hint(s, t, layer)
}

/// An empty thread: where it runs, and the few keys that work from here. A
/// draft's place is still a choice, so it shows the project picker instead.
fn thread_hint(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let Some(thread) = &s.new_thread else {
        let title = s
            .project()
            .map_or("New thread", |project| project.name.as_str());
        return ui::empty(None, title, "", None, t)
            .child(shortcuts(&[("N", "New thread"), ("K", "Commands")], t));
    };
    let choose_harness = !thread.sending
        && thread.harness.is_none()
        && s.harnesses.len() > 1
        && s.harnesses.iter().all(|harness| !harness.default);
    // `ui::empty`'s column, with the picker row where its title goes.
    col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S8)
        .items_center()
        .child(draft_project(s, thread, t, layer))
        .children(choose_harness.then(|| {
            text("Choose a harness below.")
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center)
        }))
        // A draft is already the new thread.
        .child(shortcuts(&[("K", "Commands")], t))
}

/// The `open_picker` value of a draft's project picker.
pub const DRAFT_PROJECT: &str = "draft-project";

/// Where a draft will run: a picker of every project and none, and an add
/// button whose new project the draft takes once the server has it
/// (`Desktop::project_added`). Fixed once the first message is on its way.
fn draft_project(s: &Desktop, thread: &NewThread, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = s.open_picker.as_deref() == Some(DRAFT_PROJECT) && !thread.sending;
    let current = s
        .project()
        .map_or("No project", |project| project.name.as_str());
    let trigger = ui::select_trigger(
        DRAFT_PROJECT,
        "Project",
        Some(current),
        open,
        ui::MODEL_TRIGGER_MAX,
        t,
    )
    .on_press(|s: &mut Desktop| {
        s.open_picker = if s.open_picker.as_deref() == Some(DRAFT_PROJECT)
            || s.new_thread.as_ref().is_none_or(|thread| thread.sending)
        {
            None
        } else {
            Some(DRAFT_PROJECT.to_owned())
        };
    });
    let mut menu = Menu::new(format!("{DRAFT_PROJECT}-picker"))
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.open_picker = None)
        .trigger(trigger);
    if open {
        let choice = |label: &str, glyph: &'static rune::Icon, id: Option<&str>| {
            let picked = thread.project.as_deref() == id;
            let id = id.map(str::to_owned);
            MenuItem::new(label)
                .icon(glyph)
                .checked(picked)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.open_picker = None;
                    tx.focus("draft", false);
                    if let Some(thread) = &mut s.new_thread {
                        thread.project.clone_from(&id);
                    }
                })
        };
        menu = menu
            .item(choice("No project", &ui::THREAD, None))
            .separator();
        menu = ui::menu_items(
            menu,
            s.projects
                .iter()
                .map(|project| choice(&project.name, &ui::FOLDER, Some(&project.path))),
        );
    }
    let add = ui::icon_button(
        &ui::FOLDER_PLUS,
        "Add project",
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(thread.sending)
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
        .label("You");
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

/// Render reasoning directly inside an open activity burst.
fn thought(key: &str, thought: &str, streaming: bool, t: &Theme) -> El {
    let label = if streaming { "Thinking" } else { "Thought" };
    col()
        .key(key.to_owned())
        .w_full()
        .pl(ACTIVITY_TEXT)
        .pr(CARD_INSET)
        .py(S4)
        .gap(S4)
        .child(ui::section(label, t))
        .child(
            text(if thought.trim().is_empty() {
                "Reasoning hidden"
            } else {
                thought
            })
            .w_full()
            .font(t.ui)
            .color(t.muted)
            .wrap()
            .selectable(true)
            .selection_color(t.k.selection()),
        )
}

fn tool_mark(tool: &Tool) -> (&'static rune::Icon, &str) {
    if let Some(call) = subagent_call(tool) {
        let label = match call {
            Call::Spawn => "Delegate task",
            Call::Message => "Message thread",
            Call::Read => "Read activity",
            Call::List => "List tasks",
            Call::Cancel => "Stop task",
        };
        return (&ui::AGENT, label);
    }
    match tool.name.as_str() {
        "bash" | "execute" | "shell" | "terminal" => (&ui::TERMINAL, "Run"),
        "read" | "read_file" => (&ui::FILE_READ, "Read"),
        "write" | "create" | "write_file" => (&ui::FILE_WRITE, "Write"),
        "edit" | "apply_patch" | "edit_file" => (&ui::FILE_EDIT, "Edit"),
        "search" | "grep" | "glob" | "web_search" => (&ui::SEARCH, "Search"),
        "tool" | "other" => (&ui::WRENCH, "Tool"),
        name => (&ui::WRENCH, name),
    }
}

fn meta(tool: &Tool, now: i64, t: &Theme) -> Option<El> {
    let faint = |words: String| text(words).font(t.small).color(t.faint);
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
        ToolStatus::Pending => Some(faint("Pending".to_owned())),
        ToolStatus::Abandoned => Some(faint("Stopped".to_owned())),
        ToolStatus::Failed => Some(text("Failed").font(t.small).color(t.danger)),
        ToolStatus::Done => None,
    }
}

fn tool_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let open = s.expanded.contains(&tool.key);
    let (mark, verb) = tool_mark(tool);
    let detail = tool.summary.as_str();
    let key = tool.key.clone();
    let header = activity("tool", icon(mark, ICON_SMALL).color(t.muted))
        .role("button")
        .label(format!("{verb} {detail}"))
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover(t.hover)
        .on_press(move |s: &mut Desktop| s.toggle(&key))
        .child(text(verb).font(t.ui_medium).color(t.text))
        .child(
            text(detail)
                .font(t.mono)
                .color(t.muted)
                .min_w(0.0)
                .truncate()
                .grow(1.0)
                .shrink()
                .help_when_truncated(detail),
        )
        .children(meta(tool, s.tool_clock, t))
        .child(
            icon(
                if open {
                    &ui::CHEVRON_OPEN
                } else {
                    &ui::CHEVRON
                },
                ICON_SMALL,
            )
            .color(t.faint),
        );
    let mut stack = col().key(tool.key.clone()).w_full().child(header);
    if open {
        stack = stack.child(output(tool, t));
    }
    stack
}

/// Hide opaque RPC results; command and read output may legitimately contain
/// JSON files or JSON printed by a shell.
fn output(tool: &Tool, t: &Theme) -> El {
    let mut inner = col().key("inner").w_full().gap(S12).padding(CARD_INSET);
    let command = is_command(&tool.name);
    let source = matches!(tool.name.as_str(), "read" | "read_file");
    let result = tool.output.trim();
    let structured = !command && !source && result.starts_with(['{', '[']);
    let readable = structured
        .then(|| serde_json::from_str::<serde_json::Value>(result).ok())
        .flatten();
    let result = if structured {
        readable
            .as_ref()
            .and_then(|record| {
                ["text", "message", "error", "output"]
                    .iter()
                    .find_map(|key| record.get(key).and_then(serde_json::Value::as_str))
            })
            .unwrap_or_default()
    } else {
        result
    };
    if let Some(input) = &tool.input
        && (command || tool.diffs.is_empty())
    {
        inner = inner.child(code_section(
            if command { "Command" } else { "Content" },
            input,
            t,
        ));
    }
    if !result.is_empty() {
        inner = inner.child(code_section("Output", result, t));
    }
    let show_locations = tool.summary.is_empty() && !tool.locations.is_empty();
    if show_locations {
        inner = inner.child(code_section("Files", &tool.locations.join("\n"), t));
    }
    if !tool.diffs.is_empty() {
        inner = inner.child(rune::memo(
            format!("{}-diffs", tool.key),
            (&tool.diffs, t.k.dark),
            || diff_sections(tool, t),
        ));
    }
    if tool.input.is_none() && result.is_empty() && !show_locations && tool.diffs.is_empty() {
        inner = inner.child(ui::section(
            if matches!(tool.status, ToolStatus::Running | ToolStatus::Pending) {
                "Waiting for output"
            } else {
                "No output"
            },
            t,
        ));
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
                diff.old_text.as_ref().map(|_| diff.path.clone()),
                Some(diff.path.clone()),
                diff.old_text.as_deref().unwrap_or_default(),
                &diff.new_text,
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
        "delegate_task" => Some(Call::Spawn),
        "thread_launch" if tool.child.is_some() => Some(Call::Spawn),
        "thread_send" => Some(Call::Message),
        "task_status" | "thread_read" => Some(Call::Read),
        "list_tasks" => Some(Call::List),
        "task_cancel" | "thread_stop" => Some(Call::Cancel),
        _ => None,
    }
}

fn local_child<'a>(s: &'a Desktop, tool: &Tool) -> Option<&'a viz_client::protocol::ThreadSummary> {
    tool.child_environment
        .as_ref()
        .is_none_or(|environment| s.environment.as_ref() == Some(environment))
        .then(|| tool.child.and_then(|id| s.summary(id)))
        .flatten()
}

fn open_child(
    s: &mut Desktop,
    child: viz_client::protocol::ThreadId,
    environment: Option<&str>,
    tx: &mut Tx,
) {
    if let Some(environment) = environment {
        s.open_in_environment(environment.to_owned(), Some(child), tx);
    } else {
        s.select(child, tx);
    }
}

/// A subagent's title: its thread's, or the call's summary.
fn subagent_title<'a>(s: &'a Desktop, tool: &'a Tool) -> &'a str {
    local_child(s, tool)
        .map(|summary| summary.title.as_str())
        .filter(|title| !title.is_empty())
        .unwrap_or(&tool.summary)
}

/// Where a subagent stands: its thread's live status, else its call's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Working,
    Done,
    Failed,
    Stopped,
}

impl Agent {
    fn of(s: &Desktop, tool: &Tool) -> Self {
        let status = local_child(s, tool).map(|thread| thread.status);
        match (status, tool.status) {
            (Some(ThreadStatus::Working), _)
            | (None, ToolStatus::Running | ToolStatus::Pending) => Self::Working,
            (Some(ThreadStatus::Failed), _) | (None, ToolStatus::Failed) => Self::Failed,
            (None, ToolStatus::Abandoned) => Self::Stopped,
            _ => Self::Done,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
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

fn agent_marks<'a>(
    agents: impl Iterator<Item = &'a viz_client::protocol::ThreadSummary>,
    total: usize,
    t: &Theme,
) -> El {
    const MARKS: usize = 3;
    let mut stack = row().center_y().a11y_hidden();
    for thread in agents.take(MARKS) {
        let mark = ui::harness_icon(&thread.harness);
        stack = stack.child(
            el().size(S24, S24)
                .ml(-S4)
                .radius(S12)
                .fill(t.surface)
                .center()
                .child(icon(mark, ICON_SMALL).color(t.muted)),
        );
    }
    if total > MARKS {
        stack = stack.child(
            text(format!("+{}", total - MARKS))
                .font(t.small)
                .color(t.muted)
                .ml(S4),
        );
    }
    stack
}

fn subagent_link(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let title = subagent_title(s, tool);
    let agent = Agent::of(s, tool);
    let thread = local_child(s, tool);
    let identity = thread.map_or_else(
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
    );
    let mut link = row()
        .key(tool.key.clone())
        .role("link")
        .label(title)
        .description(agent.word())
        .w_full()
        .h(S40)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(identity)
        .child(agent.mark(t));
    if let Some(child) = tool.child {
        let environment = tool.child_environment.clone();
        link = link
            .hover(t.hover)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                open_child(s, child, environment.as_deref(), tx);
            })
            .child(icon(&ui::CHEVRON, ICON_SMALL).color(t.faint));
    }
    link
}

fn agent_stack<'a>(
    s: &Desktop,
    tools: impl Iterator<Item = &'a Tool>,
    count: usize,
    t: &Theme,
) -> El {
    let mut stack = col()
        .key("subagent-stack")
        .w_full()
        .radius(theme::RADIUS_CARD)
        .fill(t.surface)
        .padding(S4);
    if count > 1 {
        stack = stack.child(ui::section(&format!("{count} tasks"), t).px(S8).py(S4));
    }
    stack.children(tools.map(|tool| subagent_link(s, tool, t)))
}

/// Talking to, reading, listing or stopping a subagent: one activity row
/// that opens the subagent it names.
fn subagent_row(s: &Desktop, tool: &Tool, call: Call, t: &Theme) -> El {
    let verb = match call {
        Call::Spawn => "Started",
        Call::Message => "Messaged",
        Call::Read => "Read",
        Call::List => "Listed tasks",
        Call::Cancel => "Stopped",
    };
    let name = (call != Call::List).then(|| subagent_title(s, tool).to_owned());
    let failed = tool.status == ToolStatus::Failed;
    let label = name
        .as_ref()
        .map_or_else(|| verb.to_owned(), |name| format!("{verb} {name}"));
    let mut line = activity(
        &tool.key,
        icon(&ui::AGENT, ICON_SMALL).color(if failed { t.danger } else { t.faint }),
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
    if let Some(meta) = meta(tool, s.tool_clock, t) {
        line = line.child(meta.key("meta"));
    }
    let environment = tool.child_environment.clone();
    match tool.child {
        Some(child) => line
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .help("Open thread")
            .hover(t.hover)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                open_child(s, child, environment.as_deref(), tx);
            }),
        None => line,
    }
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
