//! Keyboard launcher for desktop commands, threads, and transcript search.

use std::time::Duration;

use rune::{
    Color, El, Icon, Key, KeyEvent, ScrollSpec, Span, TextEdit, Tx, col, el, icon, rich, row, text,
};
use rune_kit::{Keybind, TextInput};
use viz_client::protocol::{SearchHit, ThreadId, ThreadSummary};

use crate::{
    app::{Desktop, thread_title},
    server::{Command, ThreadChange},
    sidebar,
    theme::{DIALOG_INSET, HAIRLINE, ICON, PALETTE_WIDTH, S2, S4, S8, S16, S24, S32, Size, Theme},
    ui,
};

const FIELD: &str = "palette-query";
const LIST: &str = "palette-results";
const SEARCH_DELAY: Duration = Duration::from_millis(180);
const RECENT_LIMIT: usize = 6;
const CHAT_LIMIT: usize = 24;
/// The result list's height: eight one-line rows.
const LIST_HEIGHT: f32 = S32 * 8.0;

#[derive(Debug, Default)]
enum Search {
    #[default]
    Idle,
    Debouncing(f64),
    Pending,
    Complete(Result<Vec<SearchHit>, String>),
}

/// Application-owned state for the modal and its current search request.
#[derive(Debug, Default)]
pub struct Palette {
    open: bool,
    input: TextEdit,
    filter: String,
    active: Option<Choice>,
    request: u64,
    search: Search,
}

impl Palette {
    /// Whether the modal owns keyboard input.
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Accepts a server result only while its request still owns the search.
    pub fn set_hits(&mut self, request: u64, result: Result<Vec<SearchHit>, String>) {
        if self.open && self.request == request && matches!(self.search, Search::Pending) {
            self.search = Search::Complete(result);
        }
    }

    fn invalidate(&mut self) {
        self.request = self.request.wrapping_add(1);
        self.search = Search::Idle;
    }

    fn debounce(&mut self, clock: f64, tx: &mut Tx) {
        self.search = Search::Debouncing(clock + SEARCH_DELAY.as_secs_f64());
        tx.wake_after(SEARCH_DELAY);
    }

    fn hits(&self) -> &[SearchHit] {
        match &self.search {
            Search::Complete(Ok(hits)) => hits,
            _ => &[],
        }
    }
}

/// Opens or dismisses the palette, without depending on sidebar visibility.
pub fn toggle(s: &mut Desktop, tx: &mut Tx) {
    if s.palette.is_open() {
        close(s);
    } else {
        s.palette.open = true;
        tx.focus_when_shown(FIELD, false);
    }
}

/// Shows the palette in a preview scene, with `query` typed and its message
/// hits in.
pub fn preview(s: &mut Desktop, query: &str, hits: Vec<SearchHit>) {
    let palette = &mut s.palette;
    palette.open = true;
    palette.input = TextEdit::new(query);
    palette.input.focused = true;
    palette.filter = query.trim().to_lowercase();
    palette.search = Search::Complete(Ok(hits));
}

fn close(s: &mut Desktop) {
    let palette = &mut s.palette;
    palette.open = false;
    palette.input = TextEdit::new("");
    palette.filter.clear();
    palette.active = None;
    palette.invalidate();
}

fn changed(s: &mut Desktop, tx: &mut Tx) {
    let connected = s.connected();
    let palette = &mut s.palette;
    if !palette.open {
        return;
    }
    palette.filter = palette.input.text.trim().to_lowercase();
    palette.active = None;
    palette.invalidate();
    if connected && !palette.filter.is_empty() {
        palette.debounce(tx.clock(), tx);
    }
    tx.scroll_to_instant(LIST, 0.0, 0.0);
}

/// Sends the most recent settled query; edits and disconnects invalidate old
/// replies.
pub fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    if !s.palette.open || s.palette.filter.is_empty() {
        return false;
    }
    if !s.connected() {
        if matches!(s.palette.search, Search::Idle) {
            return false;
        }
        s.palette.invalidate();
        return true;
    }
    match s.palette.search {
        Search::Idle => s.palette.debounce(clock, tx),
        Search::Debouncing(due) if clock >= due => {
            let request = s.palette.request;
            let query = s.palette.input.text.trim().to_owned();
            s.palette.search = Search::Pending;
            if !s.send(Command::SearchThreads { request, query }, tx) {
                s.palette.search = Search::Complete(Err("The server is offline.".into()));
            }
        }
        _ => return false,
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    NewThread,
    AddProject,
    Sidebar,
    Pin,
    Archive,
    Delete,
    Stop,
}

const COMMANDS: [Local; 7] = [
    Local::NewThread,
    Local::AddProject,
    Local::Sidebar,
    Local::Pin,
    Local::Archive,
    Local::Delete,
    Local::Stop,
];

impl Local {
    fn label(self, s: &Desktop) -> &'static str {
        let thread = s.selected.and_then(|id| s.summary(id));
        match self {
            Self::NewThread => "New thread",
            Self::AddProject => "Add project",
            Self::Sidebar => "Toggle sidebar",
            Self::Pin if thread.is_some_and(|thread| thread.pinned) => "Unpin thread",
            Self::Pin => "Pin thread",
            Self::Archive if thread.is_some_and(|thread| thread.archived) => "Unarchive thread",
            Self::Archive => "Archive thread",
            Self::Delete => "Delete thread",
            Self::Stop => "Stop response",
        }
    }

    const fn glyph(self) -> &'static Icon {
        match self {
            Self::NewThread => &ui::COMPOSE,
            Self::AddProject => &ui::FOLDER,
            Self::Archive => &ui::ARCHIVE,
            Self::Sidebar => &ui::SIDEBAR,
            Self::Pin => &ui::PIN,
            Self::Delete => &ui::TRASH,
            Self::Stop => &ui::STOP,
        }
    }

    const fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::NewThread => Some("N"),
            Self::Stop => Some("."),
            _ => None,
        }
    }

    fn available(self, s: &Desktop) -> bool {
        if self == Self::Sidebar {
            return true;
        }
        if !s.connected() {
            return false;
        }
        let thread = s.selected.and_then(|id| s.summary(id));
        match self {
            Self::NewThread => s.new_thread.is_none(),
            Self::AddProject | Self::Sidebar => true,
            Self::Pin | Self::Archive | Self::Delete => s.new_thread.is_none() && thread.is_some(),
            Self::Stop => s.selected.is_some() && s.busy(),
        }
    }

    fn run(self, s: &mut Desktop, tx: &mut Tx) {
        let thread = s
            .selected
            .and_then(|id| s.summary(id))
            .map(|thread| (thread.id, thread.pinned, thread.archived));
        match self {
            Self::NewThread => s.create_here(tx),
            Self::AddProject => sidebar::open_add_project(s, tx),
            Self::Sidebar => s.sidebar.toggle(),
            Self::Pin => {
                if let Some((id, pinned, _)) = thread {
                    s.send(Command::Update(id, ThreadChange::Pin(!pinned)), tx);
                }
            }
            Self::Archive => {
                if let Some((id, _, archived)) = thread {
                    s.send(Command::Update(id, ThreadChange::Archive(!archived)), tx);
                }
            }
            Self::Delete => {
                if let Some((id, _, _)) = thread {
                    sidebar::open_delete_thread(s, id, tx);
                }
            }
            Self::Stop => s.abort(tx),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Command(Local),
    Thread(ThreadId),
    Message { thread: ThreadId, entry: u64 },
}

impl Choice {
    fn key(self) -> String {
        match self {
            Self::Command(command) => format!("palette-command-{command:?}"),
            Self::Thread(id) => format!("palette-thread-{id}"),
            Self::Message { thread, entry } => format!("palette-hit-{thread}-{entry}"),
        }
    }

    fn choose(self, s: &mut Desktop, tx: &mut Tx) {
        if !s.palette.open {
            return;
        }
        let available = match self {
            Self::Command(command) => command.available(s),
            Self::Thread(_) | Self::Message { .. } => s.connected(),
        };
        if !available {
            return;
        }
        close(s);
        match self {
            Self::Command(command) => command.run(s, tx),
            Self::Thread(id) | Self::Message { thread: id, .. } => s.select(id, tx),
        }
    }
}

#[derive(Clone, Copy)]
enum ResultRow<'a> {
    Command(Local),
    Thread(&'a ThreadSummary),
    Message(&'a SearchHit),
}

impl ResultRow<'_> {
    const fn choice(self) -> Choice {
        match self {
            Self::Command(command) => Choice::Command(command),
            Self::Thread(thread) => Choice::Thread(thread.id),
            Self::Message(hit) => Choice::Message {
                thread: hit.thread,
                entry: hit.entry.0,
            },
        }
    }

    const fn group(self, recent: bool) -> &'static str {
        match self {
            Self::Command(_) => "Commands",
            Self::Thread(_) if recent => "Recent threads",
            Self::Thread(_) => "Threads",
            Self::Message(_) => "Messages",
        }
    }
}

fn matches_filter(label: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if label.is_ascii() {
        filter.split_whitespace().all(|word| {
            label
                .as_bytes()
                .windows(word.len())
                .any(|part| part.eq_ignore_ascii_case(word.as_bytes()))
        })
    } else {
        let folded = label.to_lowercase();
        filter.split_whitespace().all(|word| folded.contains(word))
    }
}

fn rows(s: &Desktop, recent: bool) -> impl Iterator<Item = ResultRow<'_>> {
    let filter = s.palette.filter.as_str();
    let commands = COMMANDS
        .into_iter()
        .filter(move |command| command.available(s) && matches_filter(command.label(s), filter))
        .map(ResultRow::Command);
    let connected = s.connected();
    let threads = if connected { s.threads.as_slice() } else { &[] }
        .iter()
        .filter(move |thread| {
            (!recent || !thread.archived) && matches_filter(thread_title(thread), filter)
        })
        .take(if recent { RECENT_LIMIT } else { CHAT_LIMIT })
        .map(ResultRow::Thread);
    let hits = if connected { s.palette.hits() } else { &[] };
    commands
        .chain(threads)
        .chain(hits.iter().map(ResultRow::Message))
}

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx, choices: &[Choice]) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.palette.input.preedit().is_some() {
        return false;
    }
    if !matches!(ev.key, Key::Down | Key::Up | Key::Enter) {
        return false;
    }
    if choices.is_empty() {
        return true;
    }
    let active = s
        .palette
        .active
        .and_then(|choice| choices.iter().position(|&other| other == choice))
        .unwrap_or_default();
    if ev.key == Key::Enter {
        if !tx.key_repeat() {
            choices[active].choose(s, tx);
        }
    } else {
        let next = if ev.key == Key::Down {
            (active + 1) % choices.len()
        } else {
            active.checked_sub(1).unwrap_or(choices.len() - 1)
        };
        let choice = choices[next];
        s.palette.active = Some(choice);
        tx.scroll_into_view_in(LIST, choice.key());
    }
    true
}

fn snippet(marked: &str, t: &Theme) -> El {
    let one_line = |ch: char| if ch.is_control() { ' ' } else { ch };
    let mut body = String::with_capacity(marked.len());
    let mut spans = Vec::new();
    let mut previous = 0;
    let mut start = None;
    for (index, marker) in marked.match_indices(['\u{1}', '\u{2}']) {
        body.extend(marked[previous..index].chars().map(one_line));
        previous = index + 1;
        if marker == "\u{1}" {
            start = Some(body.len());
        } else if let Some(start) = start.take().filter(|&start| start < body.len()) {
            spans.push(Span::new(start, body.len()).bold().color(t.accent_text));
        }
    }
    body.extend(marked[previous..].chars().map(one_line));
    rich(body, spans).font(t.small).color(t.muted).truncate()
}

fn result_row(result: ResultRow<'_>, active: bool, s: &Desktop, t: &Theme) -> El {
    let choice = result.choice();
    let (glyph, title) = match result {
        ResultRow::Command(command) => (command.glyph(), command.label(s)),
        ResultRow::Thread(thread) => (ui::harness_icon(&thread.harness), thread_title(thread)),
        ResultRow::Message(hit) => (&ui::SEARCH, s.title(hit.thread)),
    };
    let label = text(title)
        .font(if active { t.ui_medium } else { t.ui })
        .color(if active { t.text } else { t.muted })
        .group_color(t.text)
        .truncate();
    let labels = col().grow(1.0).shrink().min_w(0.0).child(label);
    let labels = if let ResultRow::Message(hit) = result {
        labels.child(snippet(&hit.snippet, t))
    } else {
        labels
    };
    let indicator = el()
        .w(S2)
        .h(S16)
        .radius(HAIRLINE)
        .fill(if active { t.accent } else { Color::TRANSPARENT })
        .a11y_hidden();
    let mut item = row()
        .key(choice.key())
        .role("option")
        .label(title)
        .focus_selected(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .min_h(S32)
        .pl(S4)
        .pr(S8)
        .py(S4)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover_group()
        .child(indicator)
        .child(
            icon(glyph, ICON)
                .color(if active { t.accent_text } else { t.muted })
                .group_color(if active { t.accent_text } else { t.text })
                .a11y_hidden(),
        )
        .child(labels)
        .on_press_tx(move |s: &mut Desktop, tx| choice.choose(s, tx));
    if let ResultRow::Command(command) = result
        && let Some(shortcut) = command.shortcut()
    {
        item = item.child(ui::kbd(shortcut, t));
    }
    item
}

fn search_note(s: &Desktop, any_choices: bool, t: &Theme) -> Option<El> {
    let (label, detail) = if !s.connected() {
        ("Connect to search threads and messages", None)
    } else if s.palette.filter.is_empty() {
        return None;
    } else {
        match &s.palette.search {
            Search::Idle | Search::Debouncing(_) | Search::Pending => ("Searching messages…", None),
            Search::Complete(Ok(hits)) if hits.is_empty() && !any_choices => ("No results", None),
            Search::Complete(Err(error)) => ("Message search failed", Some(error)),
            Search::Complete(Ok(_)) => return None,
        }
    };
    let hint = text(label)
        .role("status")
        .font(t.small)
        .color(t.faint)
        .px(S8)
        .py(S8);
    Some(if let Some(detail) = detail {
        hint.help(detail)
    } else {
        hint
    })
}

fn footer(t: &Theme) -> El {
    row()
        .w_full()
        .h(Size::Large.height())
        .px(DIALOG_INSET)
        .gap(S16)
        .center_y()
        .children(
            [
                (&["↑", "↓"][..], "Navigate"),
                (&["↵"][..], "Open"),
                (&["Esc"][..], "Close"),
            ]
            .map(|(keys, name)| {
                row()
                    .gap(S4)
                    .center_y()
                    .child(Keybind::new(keys).build(&t.k))
                    .child(text(name).font(t.small).color(t.faint))
            }),
        )
}

/// Mounts the searchable field and its result list in the shell overlay layer.
pub fn mount(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    if !s.palette.is_open() {
        return;
    }
    let recent = s.palette.filter.is_empty();
    let mut results = rows(s, recent).peekable();
    let active = s
        .palette
        .active
        .filter(|choice| rows(s, recent).any(|result| result.choice() == *choice))
        .or_else(|| results.peek().map(|result| result.choice()));
    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label("Palette results")
        .w_full()
        .h(LIST_HEIGHT)
        .padding(S8)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let mut choices = Vec::new();
    let mut group = "";
    for result in results {
        let name = result.group(recent);
        if name != group {
            group = name;
            list = list.child(
                row()
                    .key(name)
                    .h(S24)
                    .px(S8)
                    .center_y()
                    .child(ui::section(name, t)),
            );
        }
        let choice = result.choice();
        choices.push(choice);
        list = list.child(result_row(result, active == Some(choice), s, t));
    }
    if let Some(note) = search_note(s, !choices.is_empty(), t) {
        list = list.child(note.key("search-status"));
    }
    let field = TextInput::new(&s.palette.input)
        .label("Search commands, threads, and messages")
        .placeholder("Type a command or search threads…")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Large.height())
        .active_descendant(active.map(Choice::key))
        .on_edit(|s: &mut Desktop| &mut s.palette.input)
        .on_change(changed)
        .on_key(move |s, ev, tx| on_key(s, ev, tx, &choices))
        .build(&t.k)
        .key(FIELD);
    ui::sheet(
        "palette",
        "Command palette",
        PALETTE_WIDTH,
        close,
        [
            row().w_full().px(S8).child(field),
            ui::rule(t),
            list,
            ui::rule(t),
            footer(t),
        ],
        t,
        layer,
    );
}
