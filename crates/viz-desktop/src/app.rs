//! The window's state and what the user can do to it. The backend thread fills
//! it in through the app handle; the views only read it.

use std::{collections::HashSet, time::Duration};

use rune::{TextEdit, Tx};
use rune_kit::shell::PaneState;
use tokio::sync::mpsc;
use viz_client::protocol::{ConversationId, ConversationKind, ConversationSummary};

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
    /// Every conversation as the backend lists it, titles included.
    pub conversations: Vec<ConversationSummary>,
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
            selected: None,
            thread: Thread::default(),
            expanded: HashSet::new(),
            draft,
            sidebar: PaneState::new(248.0).range(200.0, 400.0),
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

    /// Takes a new view of the selected conversation.
    pub fn set_thread(&mut self, conversation: ConversationId, thread: Thread) {
        if self.selected == Some(conversation) {
            self.thread = thread;
        }
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

    /// The conversation as listed, once the list has it.
    pub fn summary(&self, conversation: ConversationId) -> Option<&ConversationSummary> {
        self.conversations
            .iter()
            .find(|summary| summary.id == conversation)
    }

    /// The conversation's name in the sidebar and header: the backend's title,
    /// or what it is until the backend has one.
    pub fn title(&self, conversation: ConversationId) -> String {
        let summary = self.summary(conversation);
        if let Some(summary) = summary.filter(|summary| !summary.title.is_empty()) {
            return summary.title.clone();
        }
        let kind = summary.map(|summary| summary.kind).or_else(|| {
            (self.root == Some(conversation)).then_some(ConversationKind::Orchestrator)
        });
        match kind {
            Some(ConversationKind::Orchestrator) => "Orchestrator",
            Some(ConversationKind::Subagent) => "Subagent",
            Some(ConversationKind::Chat) | None => "New chat",
        }
        .to_owned()
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
