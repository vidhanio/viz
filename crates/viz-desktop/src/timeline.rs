//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.
//!
//! Activity shares a narrow icon column. Details are only shown when there
//! is something to read, and a running command stays visible in a folded burst.

use rune::{El, LoopSpec, ScrollSpec, Span, Transition, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Markdown, Menu, MenuItem, Placement, ProgressCircle,
    diff::{FileDiff, LineKind},
};
use viz_client::protocol::{Task, TaskStatus, ThreadId, ThreadStatus, ThreadSummary};

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

/// A single tool has its own disclosure; it doesn't need a second fold.
fn single_tool(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    match subagent_call(tool) {
        Some(Call::Spawn) if spawned(tool) => subagent_link(s, tool, t),
        Some(call) if call != Call::Spawn && tool.status != ToolStatus::Failed => {
            subagent_row(s, tool, call, t)
        }
        _ => tool_row(s, tool, t),
    }
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
                let activity = match item {
                    Item::Tool(tool) if count == 0 => single_tool(s, tool, t),
                    _ => {
                        let burst =
                            std::iter::once((item, streaming)).chain(items.clone().take(count));
                        activity_group(s, item.key(), &burst, t)
                    }
                };
                rows.push(Kind::Activity, activity);
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

/// What an activity burst holds, for its folded heading.
struct Tally<'a> {
    thinking: bool,
    tools: usize,
    spawns: usize,
    agents: [usize; Agent::COUNT],
    failures: usize,
    last_thought: Option<&'a str>,
    active_tool: Option<&'a Tool>,
}

impl<'a> Tally<'a> {
    fn of(s: &Desktop, burst: impl Iterator<Item = (&'a Item, bool)>) -> Self {
        let mut tally = Self {
            thinking: false,
            tools: 0,
            spawns: 0,
            agents: [0; Agent::COUNT],
            failures: 0,
            last_thought: None,
            active_tool: None,
        };
        for (item, streaming) in burst {
            match item {
                Item::Thinking { text, .. } => {
                    tally.thinking |= streaming;
                    let text = text.trim();
                    if !text.is_empty() {
                        tally.last_thought = Some(text);
                    }
                }
                Item::Tool(tool) if spawned(tool) => {
                    tally.spawns += 1;
                    tally.agents[Agent::of(s, tool, delegated_task(s, tool)).index()] += 1;
                }
                Item::Tool(tool) => {
                    tally.tools += 1;
                    tally.failures += usize::from(tool.status == ToolStatus::Failed);
                    if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) {
                        tally.active_tool = Some(tool);
                    }
                }
                _ => {}
            }
        }
        tally
    }

    const fn only_thought(&self) -> bool {
        self.tools == 0 && self.spawns == 0
    }

    fn label(&self) -> String {
        match (self.tools, self.spawns) {
            (0, 0) => if self.thinking { "Thinking" } else { "Thought" }.to_owned(),
            (0, n) => format!("Delegated {n} {}", plural(n, "task", "tasks")),
            (n, 0) => format!("Called {n} {}", plural(n, "tool", "tools")),
            (tools, agents) => format!(
                "Called {tools} {}, delegated {agents} {}",
                plural(tools, "tool", "tools"),
                plural(agents, "task", "tasks")
            ),
        }
    }

    fn mark(&self) -> &'static rune::Icon {
        if self.only_thought() {
            &ui::BRAIN
        } else if self.tools == 0 {
            &ui::AGENT
        } else {
            &ui::WRENCH
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
    let tally = Tally::of(s, burst.clone());
    let Tally {
        thinking,
        spawns: spawn_count,
        failures,
        last_thought,
        active_tool,
        ..
    } = tally;
    let only_thought = tally.only_thought();
    let can_open = !only_thought || last_thought.is_some();
    let label = tally.label();
    let mark = tally.mark();
    let mut heading = if can_open {
        disclosure(&key, mark, open, t)
    } else {
        activity("header", icon(mark, ICON_SMALL).color(t.muted))
    };
    heading = heading.label(label.clone()).child(
        text(label)
            .font(t.ui_medium)
            .color(t.text)
            .truncate()
            .shrink(),
    );
    let preview = if can_open {
        last_thought
            .filter(|_| !open && (only_thought || thinking))
            .and_then(|thought| thought.lines().next())
    } else {
        (!thinking).then_some("Reasoning hidden")
    };
    heading = heading.child(
        text(preview.unwrap_or_default())
            .font(t.ui)
            .color(t.muted)
            .grow(1.0)
            .min_w(0.0)
            .shrink()
            .truncate(),
    );
    if spawn_count > 0 {
        let marks = burst.clone().filter_map(|(item, _)| match item {
            Item::Tool(tool) if spawned(tool) => {
                Some(local_child(s, tool, delegated_task(s, tool)))
            }
            _ => None,
        });
        heading = heading.child(agent_marks(marks, spawn_count, t));
    }
    if let Some(status) = Agent::summary(&tally.agents) {
        heading = heading.child(
            text(status)
                .font(t.small)
                .color(t.muted)
                .shrink()
                .truncate(),
        );
    }
    if failures > 0 {
        heading = heading.child(
            text(format!("{failures} failed"))
                .font(t.small)
                .color(t.danger),
        );
    }
    if thinking {
        heading = heading.child(ui::spinner(t));
    }
    let mut group = col()
        .key(key)
        .w_full()
        .child(heading.children(can_open.then(|| chevron(open, t))));
    if can_open && open {
        group = group.child(activity_details(s, burst, t));
    } else if let Some(tool) = active_tool {
        // Keep the current command visible even when the burst is folded.
        group = group.child(single_tool(s, tool, t).ml(S8));
    }
    group
}

/// An open activity group: reasoning, tools, and runs of delegated tasks.
fn activity_details<'a>(
    s: &Desktop,
    burst: &(impl Iterator<Item = (&'a Item, bool)> + Clone),
    t: &Theme,
) -> El {
    let mut details = col().key("details").w_full().pl(S8).gap(S2);
    let mut entries = burst.clone().peekable();
    while let Some((item, streaming)) = entries.next() {
        match item {
            Item::Thinking { key, text } => {
                let text = text.trim();
                if !text.is_empty() {
                    details = details.child(thought(key, text, streaming, t));
                }
            }
            Item::Tool(tool) if spawned(tool) => {
                let others = std::iter::from_fn(|| match entries.peek() {
                    Some((Item::Tool(tool), _)) if spawned(tool) => {
                        let tool = tool.as_ref();
                        entries.next();
                        Some(tool)
                    }
                    _ => None,
                });
                details = details.child(agent_stack(
                    s,
                    std::iter::once(tool.as_ref()).chain(others),
                    t,
                ));
            }
            Item::Tool(tool) => details = details.child(single_tool(s, tool, t)),
            _ => {}
        }
    }
    details
}

const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

fn spawned(tool: &Tool) -> bool {
    subagent_call(tool) == Some(Call::Spawn) && tool.status != ToolStatus::Failed
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
fn disclosure(key: &str, mark: &'static rune::Icon, open: bool, t: &Theme) -> El {
    let toggle = key.to_owned();
    activity("header", icon(mark, ICON_SMALL).color(t.muted))
        .role("button")
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover(t.hover)
        .on_press(move |s: &mut Desktop| s.toggle(&toggle))
}

fn chevron(open: bool, t: &Theme) -> El {
    icon(
        if open {
            &ui::CHEVRON_OPEN
        } else {
            &ui::CHEVRON
        },
        ICON_SMALL,
    )
    .color(t.faint)
    .a11y_hidden()
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

/// Keep the trace in the work-log lane without a second heading or the
/// runner's leading/trailing blank lines.
fn thought(key: &str, thought: &str, streaming: bool, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .px(CARD_INSET)
        .py(S4)
        .gap(S8)
        .items_start()
        .role("group")
        .label(if streaming { "Thinking" } else { "Thought" })
        .child(ui::lead(
            icon(&ui::BRAIN, ICON_SMALL).color(t.faint),
            t.ui.line_height,
        ))
        .child(
            Markdown::new(thought.to_owned())
                .style(t.markdown().body_font(t.ui).text(t.muted).link(t.text))
                .streaming(streaming)
                .selectable(true)
                .on_link_tx(|_: &mut Desktop, url: &str, tx: &mut Tx| {
                    _ = tx.open_url(url);
                })
                .build(&t.k)
                .grow(1.0)
                .min_w(0.0),
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
    let expandable = !tool.output.is_empty() || !tool.diffs.is_empty();
    let open = expandable && s.expanded.contains(&tool.key);
    let (mark, verb) = tool_mark(tool);
    let command = is_command(&tool.name);
    let detail = tool.summary.as_str();
    let mut summary = text(detail)
        .font(if command { t.mono } else { t.ui })
        .color(t.muted)
        .min_w(0.0)
        .truncate()
        .grow(1.0)
        .shrink();
    if let Some(full_command) = tool
        .input
        .as_deref()
        .filter(|input| command && *input != detail)
    {
        summary = summary.help(full_command);
    } else {
        summary = summary.help_when_truncated(detail);
    }
    let mut header = activity("tool", icon(mark, ICON_SMALL).color(t.muted))
        .label(format!("{verb} {detail}"))
        .child(text(verb).font(t.ui_medium).color(t.text))
        .child(summary)
        .children(meta(tool, s.tool_clock, t));
    if expandable {
        let key = tool.key.clone();
        header = header
            .role("button")
            .expanded(open)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .hover(t.hover)
            .on_press(move |s: &mut Desktop| s.toggle(&key))
            .child(chevron(open, t));
    }
    let mut stack = col().key(tool.key.clone()).w_full().child(header);
    if open {
        stack = stack.child(output(tool, t));
    }
    stack
}

fn output(tool: &Tool, t: &Theme) -> El {
    let mut inner = col().key("inner").w_full().gap(S8).px(S12).py(S8);
    if !tool.output.is_empty() {
        let literal = is_command(&tool.name) || matches!(tool.name.as_str(), "read" | "read_file");
        inner = inner.child(if literal {
            ui::code(&tool.output, t)
        } else {
            text(&tool.output)
                .w_full()
                .font(t.ui)
                .color(t.text)
                .wrap_anywhere()
                .selectable(true)
                .selection_color(t.k.selection())
        });
    }
    if !tool.diffs.is_empty() {
        inner = inner.child(rune::memo(
            format!("{}-diffs", tool.key),
            (&tool.diffs, t.k.dark),
            || diff_sections(tool, t),
        ));
    }
    col()
        .key("body")
        .w_full()
        .pl(ACTIVITY_TEXT)
        .pr(CARD_INSET)
        .pb(S4)
        .child(
            Card::Surface
                .frame(t)
                .max_h(BLOCK_HEIGHT)
                .min_h(0.0)
                .clip_rounded(theme::RADIUS_CARD)
                .scroll(ScrollSpec {
                    stick_bottom: tool.status == ToolStatus::Running,
                })
                .child(inner),
        )
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

fn child_link<'a>(tool: &'a Tool, task: Option<&'a Task>) -> Option<(ThreadId, Option<&'a str>)> {
    task.and_then(|task| {
        task.thread
            .map(|child| (child, Some(task.environment.as_str())))
    })
    .or_else(|| {
        tool.child
            .map(|child| (child, tool.child_environment.as_deref()))
    })
}

fn local_child<'a>(s: &'a Desktop, tool: &Tool, task: Option<&Task>) -> Option<&'a ThreadSummary> {
    let (child, environment) = child_link(tool, task)?;
    environment
        .is_none_or(|environment| s.environment.as_deref() == Some(environment))
        .then(|| s.summary(child))
        .flatten()
}

fn delegated_task<'a>(s: &'a Desktop, tool: &Tool) -> Option<&'a Task> {
    if let Some(id) = &tool.task_id {
        return s
            .tasks
            .iter()
            .find(|task| task.id == *id && Some(task.parent) == s.selected);
    }
    let child = tool.child?;
    let environment = tool
        .child_environment
        .as_deref()
        .or(s.environment.as_deref());
    s.tasks.iter().find(|task| {
        Some(task.parent) == s.selected
            && task.thread == Some(child)
            && Some(task.environment.as_str()) == environment
    })
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

/// Prefer the child's title over the initial task title or the call summary.
fn subagent_title<'a>(
    tool: &'a Tool,
    task: Option<&'a Task>,
    child: Option<&'a ThreadSummary>,
) -> &'a str {
    child
        .map(|summary| summary.title.as_str())
        .filter(|title| !title.is_empty())
        .or_else(|| task.map(|task| task.title.as_str()))
        .unwrap_or(&tool.summary)
}

/// Where a subagent stands: its thread's live status, else its call's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Agent {
    Working,
    Queued,
    Done,
    Failed,
    Stopped,
    Unreachable,
}

impl Agent {
    const ALL: [Self; 6] = [
        Self::Working,
        Self::Queued,
        Self::Done,
        Self::Failed,
        Self::Stopped,
        Self::Unreachable,
    ];
    const COUNT: usize = Self::ALL.len();

    const fn index(self) -> usize {
        self as usize
    }

    fn of(s: &Desktop, tool: &Tool, task: Option<&Task>) -> Self {
        if let Some(task) = task {
            if !task.reachable {
                return Self::Unreachable;
            }
            return match task.status {
                TaskStatus::Running => Self::Working,
                TaskStatus::Queued => Self::Queued,
                TaskStatus::Completed => Self::Done,
                TaskStatus::Failed => Self::Failed,
                TaskStatus::Cancelled => Self::Stopped,
            };
        }
        let status = local_child(s, tool, None).map(|thread| thread.status);
        match (status, tool.status) {
            (Some(ThreadStatus::Working), _) | (None, ToolStatus::Running) => Self::Working,
            (Some(ThreadStatus::Failed), _) | (None, ToolStatus::Failed) => Self::Failed,
            (None, ToolStatus::Abandoned) => Self::Stopped,
            (None, ToolStatus::Pending) => Self::Queued,
            (None, ToolStatus::Done) if tool.child.is_none() => Self::Queued,
            _ => Self::Done,
        }
    }

    fn summary(counts: &[usize; Self::COUNT]) -> Option<String> {
        use std::fmt::Write as _;

        let mut result = String::new();
        for (agent, &count) in Self::ALL.iter().zip(counts) {
            if count == 0 {
                continue;
            }
            let word = match agent {
                Self::Working => "running",
                Self::Queued => "queued",
                Self::Done => "done",
                Self::Failed => "failed",
                Self::Stopped => "stopped",
                Self::Unreachable => "unreachable",
            };
            if !result.is_empty() {
                result.push_str(" · ");
            }
            _ = write!(result, "{count} {word}");
        }
        (!result.is_empty()).then_some(result)
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Working => "Running",
            Self::Queued => "Queued",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Unreachable => "Unreachable",
        }
    }

    fn mark(self, t: &Theme) -> El {
        match self {
            Self::Working => ui::spinner(t),
            Self::Queued => ui::dot(t.muted),
            Self::Done => ui::dot(t.success),
            Self::Failed => ui::dot(t.danger),
            Self::Stopped | Self::Unreachable => ui::dot(t.faint),
        }
    }
}

fn agent_marks<'a>(
    agents: impl Iterator<Item = Option<&'a ThreadSummary>>,
    total: usize,
    t: &Theme,
) -> El {
    const MARKS: usize = 3;
    let mut stack = row().center_y().a11y_hidden();
    for thread in agents.take(MARKS) {
        let mark = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
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
    let task = delegated_task(s, tool);
    let thread = local_child(s, tool, task);
    let title = subagent_title(tool, task, thread);
    let title = if title.is_empty() {
        "Delegated task"
    } else {
        title
    };
    let agent = Agent::of(s, tool, task);
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
        .description(agent.word())
        .w_full()
        .min_h(S40)
        .py(S2)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .child(identity)
        .child(agent.mark(t))
        .child(text(agent.word()).font(t.small).color(t.muted));
    if let Some((child, environment)) =
        child_link(tool, task).filter(|_| task.is_none_or(|task| task.reachable))
    {
        let environment = environment.map(str::to_owned);
        link = link
            .role("link")
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

fn agent_stack<'a>(s: &Desktop, tools: impl Iterator<Item = &'a Tool>, t: &Theme) -> El {
    let mut tools = tools.peekable();
    let key = tools.peek().map_or("tasks", |tool| tool.key.as_str());
    col()
        .key(format!("{key}-stack"))
        .w_full()
        .radius(theme::RADIUS_CARD)
        .fill(t.surface)
        .padding(S4)
        .children(tools.map(|tool| subagent_link(s, tool, t)))
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
    let task = delegated_task(s, tool);
    let thread = local_child(s, tool, task);
    let mark = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
    let name = (tool.child.is_some() || task.is_some())
        .then(|| subagent_title(tool, task, thread))
        .filter(|title| !title.is_empty());
    let verb = match (call, name) {
        (Call::Read, None) => "Checked thread activity",
        (Call::Message, None) => "Sent a message",
        (Call::Cancel, None) => "Stopped a task",
        _ => verb,
    };
    let label = name.map_or_else(|| verb.to_owned(), |name| format!("{verb} {name}"));
    let mut line = activity(&tool.key, icon(mark, ICON_SMALL).color(t.muted))
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
    if name.is_none() {
        line = line.child(el().grow(1.0).a11y_hidden());
    }
    if let Some(meta) = meta(tool, s.tool_clock, t) {
        line = line.child(meta.key("meta"));
    }
    let environment = tool.child_environment.clone();
    match tool.child {
        Some(child) if task.is_none_or(|task| task.reachable) => line
            .role("link")
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .help("Open thread")
            .hover(t.hover)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                open_child(s, child, environment.as_deref(), tx);
            }),
        _ => line,
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
