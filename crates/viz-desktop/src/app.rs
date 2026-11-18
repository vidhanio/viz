//! The window's state and what the user can do to it. The backend thread fills
//! it in through the app handle; the views only read it.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use rune::{TextEdit, Tx};
use rune_kit::shell::PaneState;
use tokio::sync::mpsc;
use viz_client::protocol::ConversationId;

use crate::{backend::Command, thread::Thread};

/// How long a toast stays up.
const TOAST_LIFE: Duration = Duration::from_secs(6);
/// The most toasts shown at once; older ones go first.
const TOAST_MAX: usize = 3;

/// The backend connection, as the sidebar footer shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Trying to connect.
    Connecting,
    /// Connected to a backend of this version.
    Connected {
        /// Backend version.
        version: String,
    },
    /// Not connected, and why.
    Error(String),
}

/// A conversation other than the orchestrator, as listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed {
    /// Its id.
    pub id: ConversationId,
    /// The conversation whose task created it: a subagent's parent.
    pub owner: Option<ConversationId>,
}

/// A transient message in the bottom-right corner.
#[derive(Debug, Clone)]
pub struct Toast {
    /// Element key.
    pub id: u64,
    /// One line.
    pub title: String,
    /// More, if there is any.
    pub detail: Option<String>,
    /// When it goes, on the window clock.
    due: f64,
}

/// The window's state.
#[derive(Debug)]
pub struct Desktop {
    commands: mpsc::UnboundedSender<Command>,
    /// The connection.
    pub status: Status,
    /// Where the backend is, once the config is read.
    pub endpoint: Option<String>,
    /// The orchestrator's conversation, once connected.
    pub root: Option<ConversationId>,
    /// Every other conversation, newest first.
    pub conversations: Vec<Listed>,
    /// Titles learned along the way: a subagent's task, a chat's opening
    /// message.
    pub titles: HashMap<ConversationId, String>,
    /// The conversation in the main pane.
    pub selected: Option<ConversationId>,
    /// Its transcript and live state, once its view arrives.
    pub thread: Thread,
    /// Timeline rows the user opened (tool calls, thinking), by key.
    pub expanded: HashSet<String>,
    /// The composer's text.
    pub draft: TextEdit,
    /// The sidebar's width and whether it's open.
    pub sidebar: PaneState,
    /// The subagent section is collapsed.
    pub subagents_closed: bool,
    /// The chats section is collapsed.
    pub chats_closed: bool,
    /// Toasts, oldest first.
    pub toasts: Vec<Toast>,
    next_toast: u64,
}

impl Desktop {
    /// A state sending its commands to `commands`.
    pub fn new(commands: mpsc::UnboundedSender<Command>) -> Self {
        let mut draft = TextEdit::new("");
        draft.focused = true;
        Self {
            commands,
            status: Status::Connecting,
            endpoint: None,
            root: None,
            conversations: Vec::new(),
            titles: HashMap::new(),
            selected: None,
            thread: Thread::default(),
            expanded: HashSet::new(),
            draft,
            sidebar: PaneState::new(260.0).range(180.0, 420.0),
            subagents_closed: false,
            chats_closed: false,
            toasts: Vec::new(),
            next_toast: 0,
        }
    }

    /// A state with no connection thread behind it, for `AppSpec::init`;
    /// `main` replaces it.
    pub fn detached() -> Self {
        Self::new(mpsc::unbounded_channel().0)
    }

    /// Connected to a backend.
    pub const fn connected(&self) -> bool {
        matches!(self.status, Status::Connected { .. })
    }

    /// Shows `conversation`, empty until its view arrives.
    pub fn show(&mut self, conversation: ConversationId) {
        if self.selected != Some(conversation) {
            self.selected = Some(conversation);
            self.thread = Thread::default();
            self.expanded.clear();
        }
    }

    /// Takes a new view of the selected conversation, learning the titles it
    /// reveals.
    pub fn set_thread(&mut self, conversation: ConversationId, thread: Thread) {
        if self.selected != Some(conversation) {
            return;
        }
        for (child, task) in thread.children() {
            self.titles.entry(child).or_insert(task);
        }
        if Some(conversation) != self.root
            && let Some(opening) = thread.opening()
        {
            self.titles.entry(conversation).or_insert(opening);
        }
        self.thread = thread;
    }

    fn send(&mut self, command: Command, tx: &mut Tx) {
        if self.commands.send(command).is_err() {
            self.toast(tx, "The connection thread stopped", None);
        }
    }

    /// Opens `conversation` in the main pane.
    pub fn select(&mut self, conversation: ConversationId, tx: &mut Tx) {
        self.show(conversation);
        self.draft.focused = true;
        self.send(Command::Select(conversation), tx);
    }

    /// Starts a new chat.
    pub fn create(&mut self, tx: &mut Tx) {
        self.send(Command::Create, tx);
    }

    /// Tries to connect now instead of waiting for the next attempt.
    pub fn reconnect(&mut self, tx: &mut Tx) {
        self.status = Status::Connecting;
        self.send(Command::Reconnect, tx);
    }

    /// Whether the composer has something to send and somewhere to send it.
    pub fn can_send(&self) -> bool {
        self.connected() && self.selected.is_some() && !self.draft.text.trim().is_empty()
    }

    /// Sends the composer's text to the selected conversation.
    pub fn submit(&mut self, tx: &mut Tx) {
        let text = self.draft.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let (true, Some(conversation)) = (self.connected(), self.selected) else {
            self.toast(
                tx,
                "Not connected",
                Some("The message stays in the composer until the backend is back.".into()),
            );
            return;
        };
        self.draft.clear();
        self.send(Command::Submit { conversation, text }, tx);
    }

    /// The conversation's name in the sidebar and header.
    pub fn title(&self, conversation: ConversationId) -> String {
        if self.root == Some(conversation) {
            return "Orchestrator".into();
        }
        if let Some(title) = self.titles.get(&conversation) {
            return title.clone();
        }
        let owned = self
            .conversations
            .iter()
            .any(|listed| listed.id == conversation && listed.owner.is_some());
        if owned {
            format!("Subagent #{conversation}")
        } else {
            format!("Chat #{conversation}")
        }
    }

    /// Shows a toast for [`TOAST_LIFE`].
    pub fn toast(&mut self, tx: &mut Tx, title: impl Into<String>, detail: Option<String>) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast {
            id,
            title: title.into(),
            detail,
            due: tx.clock() + TOAST_LIFE.as_secs_f64(),
        });
        if self.toasts.len() > TOAST_MAX {
            self.toasts.remove(0);
        }
        tx.wake_after(TOAST_LIFE);
    }

    /// Closes a toast early.
    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    /// Drops toasts whose time is up; true if any went.
    pub fn expire(&mut self, clock: f64) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|toast| toast.due > clock);
        self.toasts.len() != before
    }

    /// Opens or closes a timeline row.
    pub fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_owned());
        }
    }
}
