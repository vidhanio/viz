//! Transcript, activity, plan checklists, expandable tool records and
//! subagent cards, in the reading column.
//!
//! The column has one grid. Prose and the user's bubbles span it. Every
//! activity row puts its mark in an [`ICON`]-wide column at the column's left
//! edge and its words at [`INDENT`]; what a row opens (output, changes,
//! reasoning, task cards) starts at [`INDENT`] too, and everything ends at the
//! column's right edge. Rows are flat: hovering one brings its ink from muted
//! to text, never a fill.
//!
//! A burst of reasoning and tool calls folds into one row that says what
//! happened ("Thought, read 2 files and ran 3 commands"); a running command
//! stays visible under it while folded. Details are only offered when there is
//! something to read.

use rune::{
    El, Icon, ScrollFade, ScrollSpec, Span, Transition, Tx, col, el, icon, rich, row, text,
};
use rune_kit::{
    ButtonVariant, IconButtonVariant, Markdown, Menu, MenuItem, Placement, ProgressCircle,
    diff::{FileDiff, LineKind},
};
use viz_client::protocol::{RunBoundary, Task, TaskStatus, ThreadId, ThreadStatus, ThreadSummary};

use crate::{
    app::{CheckpointDialog, Desktop, NewThread, PANE_TRAVEL, Status},
    attachments,
    diff_panel::{self, DiffFile},
    header, sidebar,
    theme::{
        self, CARD_INSET, EMPTY_TOP, EMPTY_WIDTH, HAIRLINE, ICON, ICON_SMALL, RADIUS_CONTROL,
        RADIUS_INNER, S2, S4, S6, S8, S12, S16, S24, S32, Size, Theme,
    },
    thread::{Item, PlanStep, TaskResult, Tool, ToolStatus, TurnEnd, User, is_command},
    ui::{self, Card},
};

/// The tallest an expanded tool block's body gets before it scrolls.
const BLOCK_HEIGHT: f32 = 280.0;
/// An activity row's height.
const ACTIVITY_ROW: f32 = Size::Small.height();
/// Where an activity row's words start, and where what it opens starts: the
/// mark's column and a gap.
const INDENT: f32 = ICON + S8;
/// The most diff lines an expanded edit draws; the copy action still copies
/// the whole patch.
const DIFF_LINES: usize = 400;
/// The widest a user bubble gets, as a share of the reading column.
const BUBBLE_FRAC: f32 = 0.8;
/// How long a thread may take to arrive before the timeline says it's
/// loading, in seconds: most arrive sooner, and a spinner that flashes for a
/// frame reads as a glitch.
const LOADING_DELAY: f32 = 0.4;

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
        (Kind::Activity, Kind::Activity) => S4,
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

/// The scrolling timeline, or what to show in its place. Each thread (and
/// draft) is its own keyed page with its own scroll, so changing threads
/// slides the old page out and the new one in the way [`crate::app::Switch`]
/// says.
pub fn timeline(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let thread = &s.thread;
    let empty = thread.items.is_empty() && thread.before.is_none() && !s.busy();
    let body = if s.loading && s.connected() {
        loading(t)
    } else if empty {
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
    let body = ui::reading(body)
        .key("timeline-body")
        .min_h(0.0)
        .pt(S24)
        .pb(S24);
    let page = match (s.selected, &s.new_thread) {
        (Some(id), _) => format!("page-{id}"),
        (None, Some(draft)) => format!("page-draft-{}", draft.id),
        (None, None) => "page-none".to_owned(),
    };
    col()
        .key("timeline")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(0.0)
        .child_exit(s.switch.exit(PANE_TRAVEL))
        .child(
            col()
                .key(page)
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .transition(s.switch.enter(PANE_TRAVEL))
                .scroll(ScrollSpec { stick_bottom: true })
                .scroll_fade(ScrollFade::strength(S24).linear())
                .scroll_anchor()
                .child(body),
        )
}

/// A thread whose transcript hasn't arrived: nothing, then a quiet radar
/// spinner and caption if it takes a while.
fn loading(t: &Theme) -> El {
    col()
        .key("loading")
        .w_full()
        .pt(EMPTY_TOP)
        .items_center()
        .gap(S8)
        .role("status")
        .label("Opening transcript")
        .child(ui::radar(ICON, t.accent, t))
        .child(text("Opening transcript…").font(t.small).color(t.faint))
        .transition(Transition::opacity())
        .enter_delay(LOADING_DELAY)
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

/// A single tool has its own disclosure; it doesn't need a second fold. A
/// lone delegation is a task card.
fn single_tool(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    match subagent_call(tool) {
        Some(Call::Spawn) if spawned(tool) => agent_stack(s, std::iter::once(tool), t),
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
            Item::User(user) => rows.push(Kind::Mine, mine(s, user, t)),
            Item::Text { key, text } => {
                let mut content = prose(key, text, streaming, t);
                if let Some(end) = items
                    .clone()
                    .take_while(|(item, _)| !matches!(item, Item::User(_)))
                    .find_map(|(item, _)| match item {
                        Item::TurnEnd(end) => Some(end),
                        _ => None,
                    })
                {
                    content = content.hover_group_shared(end.key.as_str());
                }
                rows.push(Kind::Prose, content);
            }
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
            Item::Continued { key } => rows.push(
                Kind::Activity,
                line(key, mark(&ui::AGENT, t))
                    .child(text("Continued on its own").font(t.ui).color(t.muted)),
            ),
            Item::TurnStatus { key, text: words } => rows.push(
                Kind::Activity,
                line(key, mark(&ui::CIRCLE_STOP, t))
                    .role("status")
                    .child(text(words).font(t.ui).color(t.muted).truncate().shrink()),
            ),
            Item::TaskResult(result) => rows.push(Kind::Activity, task_result(s, result, t)),
            Item::Notice { key, text } => rows.push(Kind::Notice, notice(key, text, t)),
            Item::TurnEnd(end) => {
                if let Some(el) = turn_end(s, end, t) {
                    rows.push(Kind::Activity, el);
                }
            }
        }
    }
}

/// What a tool call does, for its verb, mark and its burst's summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Command,
    Read,
    Write,
    Edit,
    Delete,
    Move,
    Search,
    Fetch,
    Other,
}

impl Action {
    /// Summary order: what changed things first.
    const ALL: [Self; 9] = [
        Self::Edit,
        Self::Write,
        Self::Delete,
        Self::Move,
        Self::Command,
        Self::Read,
        Self::Search,
        Self::Fetch,
        Self::Other,
    ];
    const COUNT: usize = Self::ALL.len();

    fn of(tool: &Tool) -> Self {
        match tool.name.as_str() {
            name if is_command(name) => Self::Command,
            "read" | "read_file" => Self::Read,
            "write" | "create" | "write_file" => Self::Write,
            "edit" | "apply_patch" | "edit_file" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" | "grep" | "glob" | "web_search" => Self::Search,
            "fetch" | "web_fetch" => Self::Fetch,
            _ => Self::Other,
        }
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&a| a == self)
            .unwrap_or_default()
    }

    /// Its target names a file.
    const fn on_file(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Write | Self::Edit | Self::Delete | Self::Move
        )
    }

    const fn icon(self) -> &'static Icon {
        match self {
            Self::Command => &ui::TERMINAL,
            Self::Read => &ui::FILE_READ,
            Self::Write => &ui::FILE_WRITE,
            Self::Edit | Self::Move => &ui::FILE_EDIT,
            Self::Delete => &ui::TRASH,
            Self::Search => &ui::SEARCH,
            Self::Fetch => &ui::GLOBE,
            Self::Other => &ui::WRENCH,
        }
    }

    /// The verb, finished or under way; `None` for a tool viz doesn't know.
    const fn verb(self, done: bool) -> Option<&'static str> {
        Some(match (self, done) {
            (Self::Command, true) => "Ran",
            (Self::Command, false) => "Running",
            (Self::Read, true) => "Read",
            (Self::Read, false) => "Reading",
            (Self::Write, true) => "Wrote",
            (Self::Write, false) => "Writing",
            (Self::Edit, true) => "Edited",
            (Self::Edit, false) => "Editing",
            (Self::Delete, true) => "Deleted",
            (Self::Delete, false) => "Deleting",
            (Self::Move, true) => "Moved",
            (Self::Move, false) => "Moving",
            (Self::Search, true) => "Searched",
            (Self::Search, false) => "Searching",
            (Self::Fetch, true) => "Fetched",
            (Self::Fetch, false) => "Fetching",
            (Self::Other, _) => return None,
        })
    }

    /// `n` of these, as a clause of a burst's summary.
    fn clause(self, n: usize) -> String {
        let count = |one: &'static str, many: &'static str| format!("{n} {}", plural(n, one, many));
        match self {
            Self::Command => format!("ran {}", count("command", "commands")),
            Self::Read => format!("read {}", count("file", "files")),
            Self::Write => format!("wrote {}", count("file", "files")),
            Self::Edit => format!("edited {}", count("file", "files")),
            Self::Delete => format!("deleted {}", count("file", "files")),
            Self::Move => format!("moved {}", count("file", "files")),
            Self::Search if n == 1 => "searched".to_owned(),
            Self::Search => format!("searched {n} times"),
            Self::Fetch => format!("fetched {}", count("page", "pages")),
            Self::Other => format!("used {}", count("tool", "tools")),
        }
    }
}

/// A tool name viz doesn't know, as words: `mcp__docs__search_pages` reads
/// "Search pages".
fn humanize(name: &str) -> String {
    let name = name.rsplit("__").next().unwrap_or(name);
    let mut words = name
        .split(['_', '-', ' ', '.'])
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = words.get(..1) {
        let upper = first.to_uppercase();
        words.replace_range(..1, &upper);
    }
    words
}

/// What an activity burst holds, for its folded heading.
struct Tally<'a> {
    thoughts: usize,
    thinking: bool,
    actions: [usize; Action::COUNT],
    spawns: usize,
    /// Messages to, checks on and stops of delegated tasks.
    calls: [usize; 3],
    agents: [usize; Agent::COUNT],
    failures: usize,
    last_thought: Option<&'a str>,
    active_tool: Option<&'a Tool>,
}

impl<'a> Tally<'a> {
    fn of(s: &Desktop, burst: impl Iterator<Item = (&'a Item, bool)>) -> Self {
        let mut tally = Self {
            thoughts: 0,
            thinking: false,
            actions: [0; Action::COUNT],
            spawns: 0,
            calls: [0; 3],
            agents: [0; Agent::COUNT],
            failures: 0,
            last_thought: None,
            active_tool: None,
        };
        for (item, streaming) in burst {
            match item {
                Item::Thinking { text, .. } => {
                    tally.thoughts += 1;
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
                Item::Tool(tool)
                    if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) =>
                {
                    tally.active_tool = Some(tool);
                }
                Item::Tool(tool) => {
                    match subagent_call(tool) {
                        Some(Call::Message) => tally.calls[0] += 1,
                        Some(Call::Read | Call::List) => tally.calls[1] += 1,
                        Some(Call::Cancel) => tally.calls[2] += 1,
                        _ => tally.actions[Action::of(tool).index()] += 1,
                    }
                    tally.failures += usize::from(tool.status == ToolStatus::Failed);
                }
                _ => {}
            }
        }
        tally
    }

    fn tools(&self) -> usize {
        self.actions.iter().chain(&self.calls).sum()
    }

    fn only_thought(&self) -> bool {
        self.tools() == 0 && self.spawns == 0 && self.active_tool.is_none()
    }

    /// "Thought, read 2 files and ran 1 command": reasoning first, then each
    /// kind of settled call, then delegations. A call under way shows as its
    /// own row instead, so the summary never says it ran.
    fn label(&self) -> String {
        let mut clauses = Vec::new();
        if self.thinking {
            clauses.push("thinking".to_owned());
        } else if self.thoughts > 0 {
            clauses.push("thought".to_owned());
        }
        for action in Action::ALL {
            let n = self.actions[action.index()];
            if n > 0 {
                clauses.push(action.clause(n));
            }
        }
        if self.spawns > 0 {
            clauses.push(format!(
                "delegated {} {}",
                self.spawns,
                plural(self.spawns, "task", "tasks")
            ));
        }
        let [messages, checks, stops] = self.calls;
        if messages > 0 {
            clauses.push(format!(
                "messaged {messages} {}",
                plural(messages, "task", "tasks")
            ));
        }
        if checks > 0 {
            clauses.push("checked tasks".to_owned());
        }
        if stops > 0 {
            clauses.push(format!(
                "stopped {stops} {}",
                plural(stops, "task", "tasks")
            ));
        }
        let mut label = match clauses.as_slice() {
            [] => "working".to_owned(),
            [only] => only.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        if let Some(first) = label.get(..1) {
            let upper = first.to_uppercase();
            label.replace_range(..1, &upper);
        }
        label
    }

    /// The mark of what mattered most in the burst.
    fn mark(&self) -> &'static Icon {
        if self.only_thought() {
            return &ui::BRAIN;
        }
        if self.spawns > 0 || self.calls.iter().sum::<usize>() > 0 {
            return &ui::AGENT;
        }
        Action::ALL
            .into_iter()
            .find(|action| self.actions[action.index()] > 0)
            .map_or_else(
                || {
                    if self.thoughts > 0 {
                        &ui::BRAIN
                    } else {
                        self.active_tool
                            .map_or(&ui::WRENCH, |tool| Action::of(tool).icon())
                    }
                },
                Action::icon,
            )
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
    let only_thought = tally.only_thought();
    let can_open = !only_thought || tally.last_thought.is_some();
    let label = tally.label();
    let mut heading = line("header", mark(tally.mark(), t))
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
            Item::Tool(tool) if spawned(tool) => {
                Some(local_child(s, tool, delegated_task(s, tool)))
            }
            _ => None,
        });
        heading = heading.child(agent_marks(marks, tally.spawns, t));
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

/// An open activity group: reasoning, tools, and runs of delegated tasks, in
/// the group's own icon column.
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
                details = details.child(
                    col()
                        .key(format!("{}-tasks", tool.key))
                        .w_full()
                        .pl(INDENT)
                        .py(S4)
                        .child(agent_stack(
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

const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

fn spawned(tool: &Tool) -> bool {
    subagent_call(tool) == Some(Call::Spawn) && tool.status != ToolStatus::Failed
}

/// An activity mark: a small Lucide icon in muted ink, text ink while its row
/// is hovered.
fn mark(glyph: &'static Icon, t: &Theme) -> El {
    icon(glyph, ICON_SMALL).color(t.muted).group_color(t.text)
}

/// An activity row: its mark in the icon column, then whatever follows at
/// [`INDENT`], word-spaced.
fn line(key: &str, mark: El) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(ACTIVITY_ROW)
        .gap(S4)
        .center_y()
        .child(ui::lead(mark, ACTIVITY_ROW).mr(S4))
}

/// A row that does something on press: focusable, ink-only hover.
fn pressable(row: El, t: &Theme) -> El {
    row.focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .radius(Size::Small.radius())
        .hover_group()
}

/// `row` opens and closes `key` on press, with a chevron after its words.
fn disclosure(row: El, key: &str, open: bool, t: &Theme) -> El {
    let toggle = key.to_owned();
    pressable(row, t)
        .role("button")
        .expanded(open)
        .on_press(move |s: &mut Desktop| s.toggle(&toggle))
        .child(chevron(open, t))
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
    .group_color(t.text)
    .a11y_hidden()
}

/// A quiet line while the run is working but shows nothing else that moves:
/// no reasoning or call under way and no answer streaming in.
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
            line("working", ui::spinner(t))
                .role("status")
                .label("Working")
                .child(text("Working").font(t.ui).color(t.muted)),
        );
    }
}

/// What to show when the timeline has no messages.
fn empty_state(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    match &s.status {
        Status::Error(message) => {
            let detail = if message.trim().is_empty() {
                "Start viz-server to reconnect."
            } else {
                message.as_str()
            };
            let retry = ui::button("Retry", ButtonVariant::Neutral, Size::Default)
                .on_press_tx(Desktop::reconnect)
                .build(&t.k)
                .key("retry");
            let mut body = col()
                .key("empty")
                .w_full()
                .max_w(EMPTY_WIDTH)
                .gap(S8)
                .items_center()
                .child(text("Offline").font(t.title).color(t.text))
                .child(
                    text(detail)
                        .font(t.ui)
                        .color(t.muted)
                        .wrap_anywhere()
                        .max_lines(3)
                        .align(rune::Align::Center)
                        .w_full(),
                );
            if let Some(endpoint) = &s.endpoint {
                body = body.child(
                    text(endpoint)
                        .font(t.mono_small)
                        .color(t.faint)
                        .truncate_middle()
                        .w_full()
                        .align(rune::Align::Center),
                );
            }
            return body.child(retry.mt(S8));
        }
        Status::Connecting => return ui::empty(None, "Connecting…", "", None, t),
        Status::Connected { .. } => {}
    }
    if s.selected.is_none() && s.new_thread.is_none() {
        let new_thread = ui::button("New thread", ButtonVariant::Submit, Size::Large)
            .icon(&ui::COMPOSE)
            .on_press_tx(Desktop::create_here)
            .build(&t.k);
        let (detail, actions) = if s.projects.is_empty() {
            let add_project = ui::button("Add project", ButtonVariant::Neutral, Size::Large)
                .icon(&ui::FOLDER_PLUS)
                .on_press_tx(sidebar::open_add_project)
                .build(&t.k);
            (
                "Start a thread or add a project folder.",
                row().gap(S8).child(new_thread).child(add_project),
            )
        } else {
            ("Select a thread or start a new one.", new_thread)
        };
        return ui::empty(None, "No thread selected", detail, Some(actions), t);
    }
    thread_hint(s, t, layer)
}

/// An empty thread or fresh draft: on a draft, just project and worktree
/// selection; on an existing empty thread, only the project name.
fn thread_hint(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let Some(thread) = &s.new_thread else {
        let title = s
            .project()
            .map_or("New thread", |project| project.name.as_str());
        return ui::empty(None, title, "", None, t);
    };
    let choose_harness = !thread.sending
        && thread.harness.is_none()
        && s.harnesses.len() > 1
        && s.harnesses.iter().all(|harness| !harness.default);
    col()
        .key("empty")
        .w_full()
        .max_w(EMPTY_WIDTH)
        .gap(S12)
        .items_center()
        .child(draft_project(s, thread, t, layer))
        .children(draft_worktree_controls(s, thread, t, layer))
        .children(choose_harness.then(|| {
            text("Choose a harness below.")
                .font(t.ui)
                .color(t.muted)
                .wrap()
                .align(rune::Align::Center)
        }))
}

const DRAFT_BASE_BRANCH: &str = "draft-base-branch";

/// Local vs Worktree toggle and base-branch selector shown on a fresh draft
/// when its chosen project is a Git repository.
fn draft_worktree_controls(
    s: &Desktop,
    thread: &NewThread,
    t: &Theme,
    layer: &mut Vec<El>,
) -> Option<El> {
    let project = s.project()?;
    let git = project.git.as_ref()?;
    let worktree = thread.worktree.as_ref();
    let active = worktree.is_some();
    let mut mode_chip = row()
        .key("draft-worktree-toggle")
        .role("switch")
        .label("New linked worktree")
        .checked(active)
        .h(Size::Default.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(theme::RADIUS_CONTROL)
        .fill(if active { t.accent } else { t.surface })
        .disabled(thread.sending)
        .focusable(!thread.sending)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(0.97)
        .on_press_tx(Desktop::toggle_draft_worktree);
    if !active {
        mode_chip = mode_chip.border(HAIRLINE, t.hairline);
    }
    let mode_chip = mode_chip
        .child(
            icon(
                if active {
                    &ui::FOLDER_GIT_2
                } else {
                    &ui::GIT_BRANCH
                },
                ICON_SMALL,
            )
            .color(if active { t.on_accent } else { t.muted })
            .group_color(if active { t.on_accent } else { t.text })
            .a11y_hidden(),
        )
        .child(
            text(if active {
                "New worktree"
            } else {
                "Local checkout"
            })
            .font(t.small_medium)
            .color(if active { t.on_accent } else { t.muted })
            .group_color(if active { t.on_accent } else { t.text })
            .a11y_hidden(),
        );
    let mut strip = row()
        .key("draft-git-controls")
        .gap(S8)
        .center_y()
        .child(mode_chip);
    if let Some(req) = worktree {
        strip = strip.child(draft_base_branch_picker(
            s,
            project,
            git,
            req,
            thread.sending,
            t,
            layer,
        ));
    }
    Some(strip)
}

fn draft_base_branch_picker(
    s: &Desktop,
    project: &viz_client::protocol::Project,
    git: &viz_client::protocol::GitSummary,
    req: &viz_client::protocol::WorktreeRequest,
    sending: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let current_base = req
        .base_ref
        .as_deref()
        .map(short_ref)
        .map(|base| {
            if base.len() == 40 && base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                &base[..12]
            } else {
                base
            }
        })
        .or(git.branch.as_deref())
        .unwrap_or("HEAD");
    let open = s.open_picker.as_deref() == Some(DRAFT_BASE_BRANCH) && !sending;
    let path = project.path.clone();
    let trigger = ui::select_trigger(
        DRAFT_BASE_BRANCH,
        "Base branch",
        Some(&format!("from {current_base}")),
        open,
        ui::PICKER_TRIGGER_MAX,
        t,
    )
    .on_press_tx(move |s: &mut Desktop, tx| {
        if s.open_picker.as_deref() == Some(DRAFT_BASE_BRANCH) {
            s.open_picker = None;
        } else {
            s.open_picker = Some(DRAFT_BASE_BRANCH.to_owned());
            s.load_branches(&path, tx);
        }
    });
    let mut menu = Menu::new(format!("{DRAFT_BASE_BRANCH}-picker"))
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.open_picker = None)
        .trigger(trigger);
    if open {
        let is_default = req.base_ref.is_none();
        menu = menu.item(
            MenuItem::new(format!(
                "Current HEAD ({})",
                git.branch.as_deref().unwrap_or("detached")
            ))
            .checked(is_default)
            .on_select_tx(|s: &mut Desktop, tx| {
                s.open_picker = None;
                tx.focus("draft", false);
                if let Some(wt) = s.new_thread.as_mut().and_then(|t| t.worktree.as_mut()) {
                    wt.base_ref = None;
                }
            }),
        );
        if let Some(branches) = s.branches.get(&project.path)
            && !branches.is_empty()
        {
            menu = menu.separator();
            menu = ui::menu_items(
                menu,
                branches.iter().map(|branch| {
                    let reference = branch.reference.clone();
                    let picked = req.base_ref.as_deref() == Some(reference.as_str());
                    MenuItem::new(short_ref(&reference))
                        .icon(&ui::GIT_BRANCH)
                        .checked(picked)
                        .on_select_tx(move |s: &mut Desktop, tx| {
                            s.open_picker = None;
                            tx.focus("draft", false);
                            if let Some(wt) =
                                s.new_thread.as_mut().and_then(|t| t.worktree.as_mut())
                            {
                                wt.base_ref = Some(reference.clone());
                            }
                        })
                }),
            );
        }
    }
    menu.build(&t.overlay, layer)
}

fn short_ref(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
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
    let trigger = draft_project_trigger(current, thread.project.is_some(), open, thread.sending, t)
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
                    s.choose_draft_project(id.as_deref(), tx);
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

fn draft_project_trigger(
    current: &str,
    has_project: bool,
    open: bool,
    disabled: bool,
    t: &Theme,
) -> El {
    row()
        .key(format!("{DRAFT_PROJECT}-trigger"))
        .role("combobox")
        .label(format!("Project: {current}"))
        .expanded(open)
        .focusable(!disabled)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(Size::Default.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .min_w(0.0)
        .max_w(ui::MODEL_TRIGGER_MAX)
        .child(
            icon(&ui::FOLDER, ICON_SMALL)
                .color(if has_project { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(current)
                .font(t.ui_medium)
                .color(if has_project { t.text } else { t.muted })
                .group_color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            icon(&ui::CHEVRON_OPEN, ICON_SMALL)
                .key("chevron")
                .icon_rotation(if open { std::f32::consts::PI } else { 0.0 })
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

/// A user message: a bubble on a neutral surface at the right of the column,
/// with a quiet hover-revealed footer for Copy, Rewind, and Fork.
/// It hugs short text and wraps long text at [`BUBBLE_FRAC`] of the column.
fn mine(s: &Desktop, user: &User, t: &Theme) -> El {
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
    let footer = user_actions(s, user, t);
    row()
        .key(format!("{}-row", user.key))
        .w_full()
        .justify_end()
        .hover_group()
        .child(el().w_frac(1.0 - BUBBLE_FRAC))
        .child(
            col()
                .min_w(0.0)
                .shrink()
                .items_end()
                .gap(S2)
                .child(message)
                .children(footer),
        )
}

fn user_actions(s: &Desktop, user: &User, t: &Theme) -> Option<El> {
    let mut actions = Vec::new();
    if !user.text.is_empty() {
        let copied = user.text.clone();
        actions.push(action_chip(
            "copy-user",
            &ui::COPY,
            "Copy",
            t.faint,
            t.text,
            move |_: &mut Desktop, tx| tx.copy(copied.clone()),
            t,
        ));
    }
    if let (Some(thread), Some(run), Some(turn)) = (s.selected, user.run, user.turn)
        && s.connected()
        && !s.busy()
    {
        if user.checkpoint {
            let prompt_text = user.text.clone();
            actions.push(action_chip(
                "rewind-user",
                &ui::UNDO_2,
                "Rewind",
                t.faint,
                t.warning,
                move |s: &mut Desktop, _| {
                    s.checkpoint = Some(CheckpointDialog::Rewind {
                        thread,
                        run,
                        turn,
                        prompt_text: prompt_text.clone(),
                    });
                },
                t,
            ));
        }
        actions.push(action_chip(
            "fork-user",
            &ui::GIT_FORK,
            "Fork",
            t.faint,
            t.text,
            move |s: &mut Desktop, tx| s.fork_run(run, RunBoundary::Start, false, tx),
            t,
        ));
    }
    if actions.is_empty() {
        return None;
    }
    Some(
        row()
            .key(format!("{}-actions", user.key))
            .h(22.0)
            .gap(S8)
            .center_y()
            .px(S4)
            .group_opacity(0.0, 1.0)
            .children(actions),
    )
}

fn action_chip(
    key: &str,
    glyph: &'static Icon,
    label: &'static str,
    rest: rune::Color,
    hover: rune::Color,
    on_press: impl Fn(&mut Desktop, &mut Tx) + 'static,
    t: &Theme,
) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label)
        .h(22.0)
        .gap(S4)
        .center_y()
        .radius(RADIUS_INNER)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(0.97)
        .on_press_tx(on_press)
        .child(
            icon(glyph, ICON_SMALL)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.small)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
}

/// Per-turn changed-files summary chip and quiet hover-revealed checkpoint
/// actions (`Copy`, `Fork`, `Fork worktree`, `Restore`).
fn turn_end(s: &Desktop, end: &TurnEnd, t: &Theme) -> Option<El> {
    let changed = end.changed_files.filter(|&count| count > 0);
    let chip = changed.map(|count| changed_files_chip(s, end, count, t));
    let footer = assistant_actions(s, end, t);
    if chip.is_none() && footer.is_none() {
        return None;
    }
    Some(
        col()
            .key(end.key.clone())
            .w_full()
            .gap(S4)
            .hover_group_shared(end.key.as_str())
            .children(chip)
            .children(footer),
    )
}

fn changed_files_chip(s: &Desktop, end: &TurnEnd, count: usize, t: &Theme) -> El {
    let run = end.run;
    let turn = end.turn;
    let chip_key = format!("{}-files", end.key);
    let open = s.expanded.contains(&chip_key);
    let toggle_key = chip_key.clone();
    let label = format!("Changed {count} {}", plural(count, "file", "files"));
    let open_diff = row()
        .key(format!("{chip_key}-open"))
        .role("button")
        .label("Open diff")
        .h(Size::Small.height())
        .px(S6)
        .gap(S4)
        .center_y()
        .radius(RADIUS_INNER)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .press_scale(0.97)
        .on_press_tx(move |s: &mut Desktop, tx| s.open_run_diff(run, turn, None, tx))
        .child(
            text("Open diff")
                .font(t.small_medium)
                .color(t.muted)
                .group_color(t.accent_text)
                .a11y_hidden(),
        )
        .child(
            icon(&ui::ARROW_UP_RIGHT, ICON_SMALL)
                .color(t.muted)
                .group_color(t.accent_text)
                .a11y_hidden(),
        );
    let header = row()
        .key(format!("{chip_key}-head"))
        .role("button")
        .label(label.clone())
        .expanded(open)
        .w_full()
        .h(30.0)
        .px(S8)
        .gap(S6)
        .center_y()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .on_press_tx(move |s: &mut Desktop, tx| s.toggle_run_files(run, turn, &toggle_key, tx))
        .child(
            icon(&ui::FILE_DIFF, ICON_SMALL)
                .color(t.worktree)
                .a11y_hidden(),
        )
        .child(text(label).font(t.small_medium).color(t.text).a11y_hidden())
        .child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .child(open_diff)
        .child(
            icon(&ui::CHEVRON, ICON_SMALL)
                .icon_rotation(if open {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.0
                })
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        );
    let mut card = col()
        .key(chip_key)
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.chip_fill)
        .border(HAIRLINE, t.hairline)
        .child(header);
    if open {
        card = card
            .child(ui::rule(t).key("changed-files-rule"))
            .child(changed_files_list(s, end, t));
    }
    card
}

fn changed_files_list(s: &Desktop, end: &TurnEnd, t: &Theme) -> El {
    let run = end.run;
    let turn = end.turn;
    if s.diff.loading_summaries.contains(&run) {
        return row()
            .key("changed-files-loading")
            .w_full()
            .px(S12)
            .py(S8)
            .gap(S8)
            .center_y()
            .child(ui::radar(ICON_SMALL, t.accent, t))
            .child(text("Loading changed files…").font(t.small).color(t.muted));
    }
    let Some(files) = s.diff.run_summaries.get(&run) else {
        return row()
            .key("changed-files-empty")
            .w_full()
            .px(S12)
            .py(S8)
            .child(text("No file list available.").font(t.small).color(t.muted));
    };
    col().key("changed-files-list").w_full().py(S4).children(
        files
            .iter()
            .take(6)
            .enumerate()
            .map(|(index, file)| changed_file_row(index, file, run, turn, t)),
    )
}

fn changed_file_row(
    index: usize,
    file: &DiffFile,
    run: viz_client::protocol::RunId,
    turn: usize,
    t: &Theme,
) -> El {
    let (letter, color) = diff_panel::change_badge(file.change, t);
    let (dir, name) = split_path(&file.path);
    let path = file.path.clone();
    row()
        .key(format!("changed-file-{index}"))
        .role("button")
        .label(format!("Inspect {}", file.path))
        .w_full()
        .h(Size::Small.height())
        .px(S12)
        .gap(S8)
        .center_y()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .on_press_tx(move |s: &mut Desktop, tx| s.open_run_diff(run, turn, Some(path.clone()), tx))
        .child(text(letter).font(t.mono_micro).color(color).a11y_hidden())
        .child(
            text(name.to_owned())
                .font(t.mono_small)
                .color(t.text)
                .truncate()
                .min_w(0.0)
                .a11y_hidden(),
        )
        .children((!dir.is_empty()).then(|| {
            text(dir.trim_end_matches('/').to_owned())
                .font(t.mono_small)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0)
                .a11y_hidden()
        }))
}

fn assistant_actions(s: &Desktop, end: &TurnEnd, t: &Theme) -> Option<El> {
    let mut actions = Vec::new();
    if !end.answer.is_empty() {
        let copied = end.answer.clone();
        actions.push(action_chip(
            "copy-answer",
            &ui::COPY,
            "Copy",
            t.faint,
            t.text,
            move |_: &mut Desktop, tx| tx.copy(copied.clone()),
            t,
        ));
    }
    if let Some(thread) = s.selected
        && s.connected()
        && !s.busy()
    {
        let run = end.run;
        let turn = end.turn;
        let changed = end.changed_files;
        actions.push(action_chip(
            "fork-answer",
            &ui::GIT_FORK,
            "Fork",
            t.faint,
            t.text,
            move |s: &mut Desktop, tx| s.fork_run(run, RunBoundary::End, false, tx),
            t,
        ));
        if changed.is_some() {
            actions.push(action_chip(
                "fork-worktree",
                &ui::GIT_BRANCH_PLUS,
                "Fork worktree",
                t.faint,
                t.worktree,
                move |s: &mut Desktop, tx| s.fork_run(run, RunBoundary::End, true, tx),
                t,
            ));
            actions.push(action_chip(
                "restore-checkpoint",
                &ui::ROTATE_CCW,
                "Restore checkpoint",
                t.faint,
                t.worktree,
                move |s: &mut Desktop, _| {
                    s.checkpoint = Some(CheckpointDialog::Restore {
                        thread,
                        run,
                        turn,
                        changed_files: changed,
                    });
                },
                t,
            ));
        }
    }
    if actions.is_empty() {
        return None;
    }
    Some(
        row()
            .key(format!("{}-actions", end.key))
            .h(22.0)
            .gap(S12)
            .center_y()
            .group_opacity(0.0, 1.0)
            .children(actions),
    )
}

/// Modal confirmation dialog for destructive checkpoint actions (Restore or
/// Rewind).
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let Some(action) = &s.checkpoint else {
        return;
    };
    let (title, subject, detail, confirm_label, variant) = match action {
        CheckpointDialog::Restore {
            turn,
            changed_files,
            ..
        } => (
            "Restore working tree?",
            format!("Before turn #{turn}"),
            match changed_files {
                Some(n) if *n > 0 => format!(
                    "Resets tracked changes back to the checkpoint before turn #{turn} ({n} {} recorded). Transcript messages stay intact.",
                    plural(*n, "file", "files")
                ),
                _ => format!(
                    "Resets tracked changes back to the checkpoint before turn #{turn}. Transcript messages stay intact."
                ),
            },
            "Restore files",
            ButtonVariant::Warning,
        ),
        CheckpointDialog::Rewind { turn, .. } => (
            "Rewind thread?",
            format!("To turn #{turn}"),
            "Restores the working tree and hides this turn and any later turns, placing the prompt back in the composer.".to_owned(),
            "Rewind",
            ButtonVariant::Warning,
        ),
    };
    let buttons = ui::dialog_actions([
        ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
            .on_press(|s: &mut Desktop| s.checkpoint = None)
            .build(&t.k),
        ui::button(confirm_label, variant, Size::Default)
            .on_press_tx(Desktop::confirm_checkpoint)
            .build(&t.k),
    ]);
    ui::dialog("checkpoint", title, Some(&detail))
        .subject(&subject)
        .build(
            |s: &mut Desktop| s.checkpoint = None,
            ui::dialog_body(None, buttons),
            t,
            layer,
        );
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

/// Reasoning: the trace beside a thin rail in the icon column, its words at
/// [`INDENT`], without the runner's leading or trailing blank lines.
fn thought(key: &str, thought: &str, streaming: bool, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .py(S4)
        .gap(S8)
        .items_stretch()
        .role("group")
        .label(if streaming { "Thinking" } else { "Thought" })
        .child(
            col()
                .w(ICON)
                .items_center()
                .a11y_hidden()
                .child(el().w(S2).grow(1.0).radius(HAIRLINE).fill(t.hairline)),
        )
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

/// `path` as its directory (with a trailing slash, or empty) and its name.
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rfind('/')
        .map_or(("", trimmed), |cut| (&trimmed[..=cut], &trimmed[cut + 1..]))
}

/// `path` from the thread's working directory, when it's inside it.
fn relative<'a>(s: &Desktop, path: &'a str) -> &'a str {
    s.state
        .as_ref()
        .and_then(|state| path.strip_prefix(state.cwd.trim_end_matches('/')))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
}

/// A file as its name in full ink and its directory after it, faint, cut
/// first when the row is narrow.
fn file_target(s: &Desktop, path: &str, t: &Theme) -> El {
    let (dir, name) = split_path(relative(s, path));
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
fn meta(tool: &Tool, now: i64, t: &Theme) -> Option<El> {
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

/// One call: its mark, a verb and its target (a file, a command in mono, a
/// query), then where it stands. Opens its output or changes when it has any.
fn tool_row(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let action = Action::of(tool);
    let expandable = !tool.output.is_empty() || !tool.diffs.is_empty();
    let open = expandable && s.expanded.contains(&tool.key);
    let done = !matches!(tool.status, ToolStatus::Pending | ToolStatus::Running);
    let generic = matches!(tool.name.as_str(), "tool" | "other" | "");
    let verb = action.verb(done).map_or_else(
        || (!generic).then(|| humanize(&tool.name)),
        |verb| Some(verb.to_owned()),
    );
    let detail = tool.summary.as_str();
    let mut glyph = icon(action.icon(), ICON_SMALL);
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
        .children(meta(tool, s.tool_clock, t));
    let mut stack = col().key(tool.key.clone()).w_full().child(header);
    if open {
        stack = stack.child(output(s, tool, t));
    }
    stack
}

/// An open call's output and changes, as cards at [`INDENT`]. An edit's
/// changes say more than its "Updated 1 file", so they replace it.
fn output(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let mut blocks = col().key("body").w_full().pl(INDENT).pt(S4).pb(S8).gap(S8);
    if tool.diffs.is_empty() {
        blocks = blocks.child(output_card(s, tool, t));
    } else {
        let cwd = s.state.as_ref().map(|state| state.cwd.as_str());
        blocks = blocks.child(rune::memo(
            format!("{}-diffs", tool.key),
            (&tool.diffs, t.k.dark, cwd),
            || diffs(s, tool, t),
        ));
    }
    blocks
}

/// A card's title band: what it holds, then a copy action at its end.
fn card_header(title: El, copy: El) -> El {
    row()
        .key("head")
        .w_full()
        .h(S32)
        .pl(CARD_INSET)
        .pr(S4)
        .gap(S8)
        .center_y()
        .child(title.grow(1.0).shrink().min_w(0.0))
        .child(copy)
}

fn copy_button(label: &str, copied: String, t: &Theme) -> El {
    ui::icon_button(&ui::COPY, label, IconButtonVariant::GhostMuted, Size::Small)
        .on_press_tx(move |_: &mut Desktop, tx: &mut Tx| tx.copy(copied.clone()))
        .build(&t.k)
        .key("copy")
}

/// A card body that scrolls past [`BLOCK_HEIGHT`], following the end while
/// the call runs.
fn card_body(body: El, follow: bool) -> El {
    col()
        .key("scroll")
        .w_full()
        .max_h(BLOCK_HEIGHT)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: follow,
        })
        .child(body)
}

/// Output: a command's or file's text in mono, anything else as prose.
fn output_card(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let command = is_command(&tool.name);
    let literal = command || Action::of(tool) == Action::Read;
    let title = if command {
        let full = tool.input.as_deref().unwrap_or(&tool.summary);
        text(format!("$ {}", full.lines().next().unwrap_or_default()))
            .font(t.mono)
            .color(t.muted)
            .truncate()
            .help_when_truncated(full)
    } else if Action::of(tool).on_file() {
        text(relative(s, &tool.summary))
            .font(t.ui)
            .color(t.muted)
            .truncate()
            .help_when_truncated(&tool.summary)
    } else {
        text("Output").font(t.ui).color(t.muted)
    };
    let output = tool.output.trim_end();
    let body = if literal {
        ui::code(output, t)
    } else {
        text(output)
            .w_full()
            .font(t.ui)
            .color(t.text)
            .wrap()
            .selectable(true)
            .selection_color(t.k.selection())
    };
    Card::Surface
        .frame(t)
        .key("output")
        .clip_rounded(theme::RADIUS_CARD)
        .child(card_header(
            title,
            copy_button("Copy output", tool.output.clone(), t),
        ))
        .child(ui::rule(t))
        .child(card_body(
            col().w_full().px(CARD_INSET).py(S8).child(body),
            tool.status == ToolStatus::Running,
        ))
}

fn diffs(s: &Desktop, tool: &Tool, t: &Theme) -> El {
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
            diff_card(s, &file, t).key(format!("diff-{index}"))
        }))
}

/// One file's changes: its name, directory and line counts over its hunks,
/// added and removed lines on their washes, changed words washed again.
fn diff_card(s: &Desktop, file: &FileDiff, t: &Theme) -> El {
    let path = file.path();
    let (dir, name) = split_path(relative(s, path));
    let count = |n: usize, sign: char, ink| {
        (n > 0).then(|| text(format!("{sign}{n}")).font(t.mono_small).color(ink))
    };
    let title = row()
        .gap(S8)
        .center_y()
        .help(path)
        .child(
            text(name)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .min_w(0.0),
        )
        .children((!dir.is_empty()).then(|| {
            text(dir.trim_end_matches('/'))
                .font(t.ui)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0)
        }))
        .children(count(file.additions(), '+', t.success))
        .children(count(file.deletions(), '−', t.danger));
    let mut body = col().w_full().py(S4);
    if file.hunks.is_empty() {
        body = body.child(
            text(if file.binary {
                "Binary change"
            } else {
                "No text changes"
            })
            .font(t.ui)
            .color(t.muted)
            .px(CARD_INSET),
        );
    }
    let total: usize = file.hunks.iter().map(|hunk| hunk.lines.len()).sum();
    let mut shown = 0;
    for (index, hunk) in file.hunks.iter().enumerate() {
        if shown == DIFF_LINES {
            break;
        }
        if index > 0 {
            body = body.child(
                text(hunk.header())
                    .font(t.mono_small)
                    .color(t.faint)
                    .px(CARD_INSET)
                    .py(S4),
            );
        }
        for line in hunk.lines.iter().take(DIFF_LINES - shown) {
            body = body.child(diff_line(line, t));
            shown += 1;
        }
    }
    if total > shown {
        body = body.child(
            text(format!("{} more lines", total - shown))
                .font(t.small)
                .color(t.faint)
                .px(CARD_INSET)
                .pt(S4),
        );
    }
    Card::Surface
        .frame(t)
        .clip_rounded(theme::RADIUS_CARD)
        .child(card_header(
            title,
            copy_button("Copy patch", file.patch(), t),
        ))
        .child(ui::rule(t))
        .child(card_body(body, false))
}

/// A diff line: its sign in a gutter, then the line, wrapping only where it
/// must and continuing under itself rather than under the sign.
fn diff_line(line: &rune_kit::diff::Line, t: &Theme) -> El {
    let (sign, sign_ink, wash, ink) = match line.kind {
        LineKind::Added => ("+", t.success, Some(t.added), t.text),
        LineKind::Removed => ("−", t.danger, Some(t.removed), t.text),
        LineKind::Context => ("", t.faint, None, t.muted),
    };
    let spans = wash.map_or_else(Vec::new, |wash| {
        line.changes
            .iter()
            .filter(|range| range.start < range.end && range.end <= line.text.len())
            .map(|range| Span {
                bg: Some(wash),
                ..Span::new(range.start, range.end)
            })
            .collect()
    });
    let mut row = row()
        .w_full()
        .px(CARD_INSET)
        .gap(S4)
        .items_start()
        .child(text(sign).font(t.mono).color(sign_ink).w(S12).a11y_hidden())
        .child(
            rich(line.text.clone(), spans)
                .font(t.mono)
                .color(ink)
                .wrap_anywhere()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    if let Some(wash) = wash {
        row = row.fill(wash);
    }
    row
}

/// The run's checklist, in the activity grid: a heading row with progress,
/// then each step's mark in the icon column.
fn plan(key: &str, steps: &[PlanStep], t: &Theme) -> El {
    let line_height = t.ui.line_height;
    let done = steps
        .iter()
        .filter(|step| step.status == ToolStatus::Done)
        .count();
    col()
        .key(key)
        .w_full()
        .role("list")
        .label("Plan")
        .gap(S4)
        .child(
            line("heading", mark(&ui::PLAN, t))
                .child(text("Plan").font(t.ui_medium).color(t.text))
                .child(
                    text(format!("{done} of {}", steps.len()))
                        .font(t.small)
                        .color(t.faint),
                ),
        )
        .children(steps.iter().enumerate().map(|(index, step)| {
            let (mark, label, ink) = match step.status {
                ToolStatus::Done => (
                    icon(&ui::CHECK, ICON_SMALL).color(t.success),
                    "Done",
                    t.muted,
                ),
                ToolStatus::Running => (ui::spinner(t), "In progress", t.text),
                _ => (
                    icon(&ui::CIRCLE, ICON_SMALL).color(t.faint),
                    "Pending",
                    t.muted,
                ),
            };
            row()
                .key(format!("step-{index}"))
                .role("listitem")
                .description(label)
                .w_full()
                .gap(S8)
                .items_start()
                .child(ui::lead(mark, line_height))
                .child(
                    text(&step.text)
                        .font(t.ui)
                        .color(ink)
                        .grow(1.0)
                        .shrink()
                        .wrap(),
                )
        }))
}

/// A delegated task's outcome arriving: its title and how it ended, opening
/// its thread; the result itself opens beneath.
fn task_result(s: &Desktop, result: &TaskResult, t: &Theme) -> El {
    let task = s.tasks.iter().find(|task| task.id == result.task);
    let title = task
        .map(|task| task.title.as_str())
        .filter(|title| !title.is_empty())
        .or_else(|| (!result.title.is_empty()).then_some(result.title.as_str()))
        .unwrap_or("Delegated task");
    let (word, ink) = match result.status.as_str() {
        "completed" => ("finished", t.muted),
        "continued" => ("continued", t.muted),
        "failed" => ("failed", t.danger),
        "cancelled" => ("was stopped", t.muted),
        _ => ("ended", t.muted),
    };
    let open = !result.detail.is_empty() && s.expanded.contains(&result.key);
    let mut heading = line("heading", mark(&ui::INBOX, t))
        .label(format!("{title} {word}"))
        .child(
            text(title)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .shrink(),
        )
        .child(text(word).font(t.ui).color(ink));
    if !result.detail.is_empty() {
        heading = disclosure(heading, &result.key, open, t);
    }
    let preview = (!open)
        .then(|| result.detail.lines().find(|line| !line.trim().is_empty()))
        .flatten();
    heading = heading.child(
        text(preview.unwrap_or_default())
            .font(t.ui)
            .color(t.faint)
            .truncate()
            .shrink()
            .grow(1.0)
            .min_w(0.0),
    );
    if let Some((child, environment)) = task
        .filter(|task| task.reachable)
        .and_then(|task| task.thread.map(|child| (child, task.environment.clone())))
    {
        heading = heading.child(
            ui::button("Open", ButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    open_child(s, child, Some(&environment), tx);
                })
                .build(&t.k)
                .key("open"),
        );
    }
    let mut block = col().key(result.key.clone()).w_full().child(heading);
    if open {
        block = block.child(
            col().key("detail").w_full().pl(INDENT).pt(S2).pb(S8).child(
                Markdown::new(result.detail.clone())
                    .style(t.markdown().body_font(t.ui).text(t.muted).link(t.text))
                    .selectable(true)
                    .on_link_tx(|_: &mut Desktop, url: &str, tx: &mut Tx| {
                        _ = tx.open_url(url);
                    })
                    .build(&t.k)
                    .w_full(),
            ),
        );
    }
    block
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

/// Up to three delegated agents' flat monochrome harness icons.
fn agent_marks<'a>(
    agents: impl Iterator<Item = Option<&'a ThreadSummary>>,
    total: usize,
    t: &Theme,
) -> El {
    const MARKS: usize = 3;
    let mut stack = row().gap(S4).center_y().a11y_hidden();
    for thread in agents.take(MARKS) {
        let glyph = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
        stack = stack.child(icon(glyph, ICON_SMALL).color(t.muted));
    }
    if total > MARKS {
        stack = stack.child(
            text(format!("+{}", total - MARKS))
                .font(t.small)
                .color(t.muted),
        );
    }
    stack
}

/// One delegated task in a task card: who runs it, what it's called, where
/// it stands; opens its thread when that's reachable.
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
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .child(identity.grow(1.0).shrink().min_w(0.0))
        .child(agent.mark(t))
        .child(text(agent.word()).font(t.small).color(t.muted));
    if let Some((child, environment)) =
        child_link(tool, task).filter(|_| task.is_none_or(|task| task.reachable))
    {
        let environment = environment.map(str::to_owned);
        link = pressable(link, t)
            .role("link")
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                open_child(s, child, environment.as_deref(), tx);
            })
            .child(chevron(false, t));
    }
    link
}

/// Delegated tasks side by side in one card, a hairline between each.
fn agent_stack<'a>(s: &Desktop, tools: impl Iterator<Item = &'a Tool>, t: &Theme) -> El {
    let mut tools = tools.peekable();
    let key = tools.peek().map_or("tasks", |tool| tool.key.as_str());
    let mut card = Card::Outline
        .frame(t)
        .key(format!("{key}-stack"))
        .role("list")
        .label("Delegated tasks");
    for (index, tool) in tools.enumerate() {
        if index > 0 {
            card = card.child(ui::rule(t).key(format!("rule-{index}")));
        }
        card = card.child(subagent_link(s, tool, t));
    }
    card
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
    let glyph = thread.map_or(&ui::AGENT, |thread| ui::harness_icon(&thread.harness));
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
    let mut row = line(&tool.key, mark(glyph, t))
        .label(label)
        .child(text(verb).font(t.ui).color(t.muted).group_color(t.text))
        .children(name.map(|name| {
            text(name)
                .key("title")
                .font(t.ui)
                .color(t.text)
                .truncate()
                .shrink()
                .min_w(0.0)
        }));
    let environment = tool.child_environment.clone();
    let link = tool
        .child
        .filter(|_| task.is_none_or(|task| task.reachable));
    if let Some(child) = link {
        row = pressable(row, t)
            .role("link")
            .help("Open thread")
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                open_child(s, child, environment.as_deref(), tx);
            })
            .child(chevron(false, t));
    }
    row.child(el().grow(1.0).min_w(0.0).a11y_hidden())
        .children(meta(tool, s.tool_clock, t).map(|meta| meta.key("meta")))
}

/// An answer that didn't come, in danger ink.
fn notice(key: &str, message: &str, t: &Theme) -> El {
    row()
        .key(key.to_owned())
        .role("alert")
        .w_full()
        .gap(S8)
        .items_start()
        .child(ui::lead(
            icon(&ui::CIRCLE_X, ICON_SMALL).color(t.danger),
            t.ui.line_height,
        ))
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
