//! Keyboard launcher for desktop commands, chats, and transcript search.

use std::time::Duration;

use rune::{El, Icon, Key, KeyEvent, ScrollSpec, Span, TextEdit, Tx, col, icon, rich, row, text};
use rune_kit::{Keybind, TextInput};
use viz_client::protocol::{ConversationId, ConversationKind, ConversationSummary, SearchHit};

use crate::{
    app::{Desktop, chat_title},
    backend::{ChatChange, Command},
    changes, sidebar,
    theme::{DIALOG_INSET, ICON, PALETTE_WIDTH, S4, S8, S16, S24, S32, Size, Theme},
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

    /// Accepts a backend result only while its request still owns the search.
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
            if !s.send(Command::SearchConversations { request, query }, tx) {
                s.palette.search = Search::Complete(Err("The backend is offline.".into()));
            }
        }
        _ => return false,
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Local {
    NewChat,
    AddProject,
    Sidebar,
    Changes,
    Commit,
    Push,
    PullRequest,
    Pin,
    Archive,
    Delete,
    Stop,
}

const COMMANDS: [Local; 11] = [
    Local::NewChat,
    Local::AddProject,
    Local::Sidebar,
    Local::Changes,
    Local::Commit,
    Local::Push,
    Local::PullRequest,
    Local::Pin,
    Local::Archive,
    Local::Delete,
    Local::Stop,
];

impl Local {
    fn label(self, s: &Desktop) -> &'static str {
        let chat = s.selected.and_then(|id| s.summary(id));
        match self {
            Self::NewChat => "New chat",
            Self::AddProject => "Add project",
            Self::Sidebar => "Toggle sidebar",
            Self::Changes => "Toggle changes",
            Self::Commit => "Commit changes",
            Self::Push => "Push branch",
            Self::PullRequest => "Create pull request",
            Self::Pin if chat.is_some_and(|chat| chat.pinned) => "Unpin chat",
            Self::Pin => "Pin chat",
            Self::Archive if chat.is_some_and(|chat| chat.archived) => "Unarchive chat",
            Self::Archive => "Archive chat",
            Self::Delete => "Delete chat",
            Self::Stop => "Stop response",
        }
    }

    const fn glyph(self) -> &'static Icon {
        match self {
            Self::NewChat => &ui::COMPOSE,
            Self::Commit | Self::PullRequest => &ui::GIT_BRANCH,
            Self::AddProject => &ui::FOLDER,
            Self::Archive => &ui::ARCHIVE,
            Self::Sidebar => &ui::SIDEBAR,
            Self::Changes => &ui::INSPECTOR,
            Self::Push => &ui::ARROW_UP,
            Self::Pin => &ui::PIN,
            Self::Delete => &ui::TRASH,
            Self::Stop => &ui::STOP,
        }
    }

    const fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::NewChat => Some("N"),
            Self::Changes => Some("D"),
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
        let chat = s.selected.and_then(|id| s.summary(id));
        match self {
            Self::NewChat => s.new_chat.is_none(),
            Self::AddProject | Self::Sidebar => true,
            Self::Changes => {
                s.changes.pane.is_open()
                    || s.selected.is_some()
                        && s.conversation
                            .as_ref()
                            .is_some_and(|state| state.git.is_some())
            }
            Self::Commit | Self::Push | Self::PullRequest => {
                s.selected.is_some()
                    && s.conversation
                        .as_ref()
                        .is_some_and(|state| state.git.is_some())
            }
            Self::Pin | Self::Archive | Self::Delete => {
                s.new_chat.is_none()
                    && chat.is_some_and(|chat| chat.kind != ConversationKind::Orchestrator)
            }
            Self::Stop => s.selected.is_some() && s.busy(),
        }
    }

    fn run(self, s: &mut Desktop, tx: &mut Tx) {
        let chat = s
            .selected
            .and_then(|id| s.summary(id))
            .map(|chat| (chat.id, chat.pinned, chat.archived));
        match self {
            Self::NewChat => s.create_here(tx),
            Self::AddProject => sidebar::open_add_project(s, tx),
            Self::Sidebar => s.sidebar.toggle(),
            Self::Changes => changes::toggle(s, tx),
            Self::Commit => changes::commit_dialog(s, tx),
            Self::Push => changes::push(s, tx),
            Self::PullRequest => changes::pr_dialog(s, tx),
            Self::Pin => {
                if let Some((id, pinned, _)) = chat {
                    s.send(Command::Update(id, ChatChange::Pin(!pinned)), tx);
                }
            }
            Self::Archive => {
                if let Some((id, _, archived)) = chat {
                    s.send(Command::Update(id, ChatChange::Archive(!archived)), tx);
                }
            }
            Self::Delete => {
                if let Some((id, _, _)) = chat {
                    sidebar::open_delete_chat(s, id, tx);
                }
            }
            Self::Stop => s.abort(tx),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Command(Local),
    Chat(ConversationId),
    Message {
        conversation: ConversationId,
        entry: u64,
    },
}

impl Choice {
    fn key(self) -> String {
        match self {
            Self::Command(command) => format!("palette-command-{command:?}"),
            Self::Chat(id) => format!("palette-chat-{id}"),
            Self::Message {
                conversation,
                entry,
            } => format!("palette-hit-{conversation}-{entry}"),
        }
    }

    fn choose(self, s: &mut Desktop, tx: &mut Tx) {
        if !s.palette.open {
            return;
        }
        let available = match self {
            Self::Command(command) => command.available(s),
            Self::Chat(_) | Self::Message { .. } => s.connected(),
        };
        if !available {
            return;
        }
        close(s);
        match self {
            Self::Command(command) => command.run(s, tx),
            Self::Chat(id)
            | Self::Message {
                conversation: id, ..
            } => s.select(id, tx),
        }
    }
}

#[derive(Clone, Copy)]
enum ResultRow<'a> {
    Command(Local),
    Chat(&'a ConversationSummary),
    Message(&'a SearchHit),
}

impl ResultRow<'_> {
    const fn choice(self) -> Choice {
        match self {
            Self::Command(command) => Choice::Command(command),
            Self::Chat(chat) => Choice::Chat(chat.id),
            Self::Message(hit) => Choice::Message {
                conversation: hit.conversation,
                entry: hit.entry.0,
            },
        }
    }

    const fn group(self, recent: bool) -> &'static str {
        match self {
            Self::Command(_) => "Commands",
            Self::Chat(_) if recent => "Recent chats",
            Self::Chat(_) => "Chats",
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
        .filter(move |command| {
            command.available(s)
                && (matches_filter(command.label(s), filter)
                    || (*command == Local::PullRequest && filter == "pr"))
        })
        .map(ResultRow::Command);
    let connected = s.connected();
    let chats = if connected {
        s.conversations.as_slice()
    } else {
        &[]
    }
    .iter()
    .filter(move |chat| (!recent || !chat.archived) && matches_filter(chat_title(chat), filter))
    .take(if recent { RECENT_LIMIT } else { CHAT_LIMIT })
    .map(ResultRow::Chat);
    let hits = if connected { s.palette.hits() } else { &[] };
    commands
        .chain(chats)
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
            spans.push(Span::new(start, body.len()).bold().color(t.accent));
        }
    }
    body.extend(marked[previous..].chars().map(one_line));
    rich(body, spans).font(t.small).color(t.muted).truncate()
}

fn result_row(result: ResultRow<'_>, active: bool, s: &Desktop, t: &Theme) -> El {
    let choice = result.choice();
    let (glyph, title) = match result {
        ResultRow::Command(command) => (command.glyph(), command.label(s)),
        ResultRow::Chat(chat) if s.root == Some(chat.id) => (&ui::STAR, chat_title(chat)),
        ResultRow::Chat(chat) if chat.kind == ConversationKind::Subagent => {
            (&ui::BOT, chat_title(chat))
        }
        ResultRow::Chat(chat) => (&ui::CHAT, chat_title(chat)),
        ResultRow::Message(hit) => (&ui::SEARCH, s.title(hit.conversation)),
    };
    let label = text(title).font(t.ui).color(t.text).truncate();
    let labels = col().grow(1.0).shrink().min_w(0.0).child(label);
    let labels = if let ResultRow::Message(hit) = result {
        labels.child(snippet(&hit.snippet, t))
    } else {
        labels
    };
    let mut item = row()
        .key(choice.key())
        .role("option")
        .label(title)
        .focus_selected(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .min_h(S32)
        .px(S8)
        .py(S4)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover(t.hover)
        .child(icon(glyph, ICON).color(t.muted).a11y_hidden())
        .child(labels)
        .on_press_tx(move |s: &mut Desktop, tx| choice.choose(s, tx));
    if active {
        item = item.fill(t.selected);
    }
    if let ResultRow::Command(command) = result
        && let Some(shortcut) = command.shortcut()
    {
        item = item.child(ui::kbd(shortcut, t));
    }
    item
}

fn search_note(s: &Desktop, any_choices: bool, t: &Theme) -> Option<El> {
    let (label, detail) = if !s.connected() {
        ("Connect to search chats and messages", None)
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
        .label("Search commands, chats, and messages")
        .placeholder("Type a command or search chats…")
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
    // No title row or close button: the field says what this is, and the
    // footer names Escape.
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
