//! The window's state and what the user can do to it. The backend thread fills
//! it in through the app handle; the views only read it.

use std::{
    collections::{HashMap, HashSet},
    mem,
    time::Duration,
};

use rune::{TextEdit, Tx};
use rune_kit::shell::PaneState;
use tokio::sync::mpsc;
use viz_client::protocol::{
    Attachment, ConversationId, ConversationKind, ConversationState, ConversationStatus,
    ConversationSummary, CreateConversation, Harness, Project, Submit, Worktree,
};

use crate::{
    attachments::{self, Loaded, Source},
    backend::{ChatChange, Command},
    changes::{self, Changes},
    composer::Completion,
    palette::Palette,
    sidebar::Navigation,
    thread::Thread,
};

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

/// A fresh composer while the ACP runner is being chosen or bound.
#[derive(Debug)]
pub struct NewChat {
    pub project: Option<String>,
    worktree: Option<Worktree>,
    pub request: Option<u64>,
}

#[derive(Debug)]
struct Draft {
    edit: TextEdit,
    attachments: Vec<Attachment>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            edit: TextEdit::new(""),
            attachments: Vec::new(),
        }
    }
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
    pub projects: Vec<Project>,
    pub harnesses: Vec<Harness>,
    /// The conversation in the main pane.
    pub selected: Option<ConversationId>,
    /// Its transcript and live state, once its view arrives.
    pub thread: Thread,
    pub history_loading: bool,
    pub conversation: Option<ConversationState>,
    pub new_chat: Option<NewChat>,
    next_create: u64,
    /// Timeline rows the user opened (tool calls, thinking), by key.
    pub expanded: HashSet<String>,
    /// The composer's text.
    pub draft: TextEdit,
    pub attachments: Vec<Attachment>,
    attachment_reads: HashSet<ConversationId>,
    drafts: HashMap<ConversationId, Draft>,
    pub completion: Completion,
    pub open_picker: Option<String>,
    pub navigation: Navigation,
    /// The sidebar's width and whether it's open.
    pub sidebar: PaneState,
    pub changes: Changes,
    pub palette: Palette,
    /// Toasts, oldest first.
    pub toasts: Vec<Toast>,
    next_toast: u64,
    /// The window has keyboard focus; notifications cover what the user can't
    /// see.
    focused: bool,
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
            projects: Vec::new(),
            harnesses: Vec::new(),
            selected: None,
            thread: Thread::default(),
            history_loading: false,
            conversation: None,
            new_chat: None,
            next_create: 0,
            expanded: HashSet::new(),
            draft,
            attachments: Vec::new(),
            attachment_reads: HashSet::new(),
            drafts: HashMap::new(),
            completion: Completion::default(),
            open_picker: None,
            navigation: Navigation::default(),
            sidebar: PaneState::new(256.0).range(220.0, 384.0),
            changes: Changes::default(),
            palette: Palette::default(),
            toasts: Vec::new(),
            next_toast: 0,
            focused: true,
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

    /// Takes a new conversation list. A turn that ended, failed or stopped for
    /// an approval where the user isn't looking becomes a desktop notification.
    pub fn set_conversations(&mut self, conversations: Vec<ConversationSummary>, tx: &mut Tx) {
        use ConversationStatus::{Approval, Failed, Idle, Working};
        for chat in &conversations {
            let Some(before) = self.summary(chat.id).map(|old| old.status) else {
                continue;
            };
            let ran = matches!(before, Working | Approval);
            let watched = self.focused && self.selected == Some(chat.id);
            let body = match chat.status {
                _ if before == chat.status || watched => continue,
                Approval => "Needs approval",
                // A subagent's end reaches its parent, which notifies in turn.
                _ if chat.kind == ConversationKind::Subagent => continue,
                Idle if ran => "Finished",
                Failed if ran => "Failed",
                _ => continue,
            };
            self.send(
                Command::Notify {
                    title: chat_title(chat).to_owned(),
                    body,
                },
                tx,
            );
        }
        self.conversations = conversations;
    }

    /// The window gained or lost focus; coming back reads the open chat.
    pub fn set_focus(&mut self, focused: bool, tx: &mut Tx) {
        self.focused = focused;
        if focused
            && self.connected()
            && let Some(id) = self
                .selected
                .filter(|&id| self.summary(id).is_some_and(|chat| chat.unread))
        {
            self.send(Command::Update(id, ChatChange::Unread(false)), tx);
        }
    }

    /// Shows `conversation`, empty until its view arrives.
    pub fn show(&mut self, conversation: ConversationId) {
        if self.selected != Some(conversation) || self.new_chat.is_some() {
            self.save_draft();
            self.new_chat = None;
            self.selected = Some(conversation);
            let draft = self.drafts.remove(&conversation).unwrap_or_default();
            self.draft = draft.edit;
            self.attachments = draft.attachments;
            self.reset_thread();
            self.navigation.reveal(conversation, &self.conversations);
        }
    }

    fn save_draft(&mut self) {
        let edit = mem::replace(&mut self.draft, TextEdit::new(""));
        let attachments = mem::take(&mut self.attachments);
        if let Some(id) = self.selected.take()
            && (!edit.text.is_empty() || !attachments.is_empty())
        {
            self.drafts.insert(id, Draft { edit, attachments });
        }
    }

    fn reset_thread(&mut self) {
        self.thread = Thread::default();
        self.history_loading = false;
        self.conversation = None;
        self.expanded.clear();
        self.completion.reset();
        self.open_picker = None;
        self.changes.reset();
    }

    /// Takes a new view of the selected conversation.
    pub fn set_thread(&mut self, conversation: ConversationId, thread: Thread) {
        if self.selected == Some(conversation) {
            self.thread = thread;
        }
    }

    pub fn set_conversation(&mut self, id: ConversationId, state: ConversationState, tx: &mut Tx) {
        if self.selected != Some(id) {
            return;
        }
        let previous = self.conversation.as_ref();
        let status_changed = previous.map(|prev| prev.status) != Some(state.status);
        let refresh_git =
            status_changed || previous.and_then(|prev| prev.git.as_ref()) != state.git.as_ref();
        self.conversation = Some(state);
        if status_changed {
            self.update_title(tx);
        }
        if refresh_git {
            changes::refresh(self, tx);
        }
    }

    pub fn update_title(&self, tx: &mut Tx) {
        tx.set_title(
            match (&self.status, self.conversation.as_ref().map(|s| s.status)) {
                (Status::Error(_), _) => "viz — Offline",
                (Status::Connecting, _) => "viz — Connecting",
                (_, Some(ConversationStatus::Working)) => "viz — Working",
                (_, Some(ConversationStatus::Approval)) => "viz — Approval needed",
                (_, Some(ConversationStatus::Failed)) => "viz — Failed",
                _ => "viz",
            },
        );
    }

    pub fn load_earlier(&mut self, tx: &mut Tx) {
        if !self.history_loading
            && self.connected()
            && let (Some(conversation), Some(before)) = (self.selected, self.thread.before)
            && self.send(
                Command::LoadEarlier {
                    conversation,
                    before,
                },
                tx,
            )
        {
            self.history_loading = true;
        }
    }

    pub fn loaded_earlier(&mut self, id: ConversationId, thread: Option<Thread>, tx: &mut Tx) {
        if self.selected != Some(id) {
            return;
        }
        self.history_loading = false;
        if let Some(thread) = thread {
            if let Some(first) = self.thread.items.first() {
                tx.keep_on_screen(first.key());
            }
            self.thread = thread;
        }
    }

    pub fn branch(&self) -> Option<&str> {
        if let Some(git) = self
            .conversation
            .as_ref()
            .and_then(|state| state.git.as_ref())
        {
            return Some(git.branch.as_deref().unwrap_or("Detached"));
        }
        self.project().and_then(|project| project.branch.as_deref())
    }

    pub fn send(&mut self, command: Command, tx: &mut Tx) -> bool {
        if self.commands.send(command).is_ok() {
            return true;
        }
        self.toast(tx, "Offline", None);
        false
    }

    /// Opens `conversation` in the main pane.
    pub fn select(&mut self, conversation: ConversationId, tx: &mut Tx) {
        self.show(conversation);
        self.update_title(tx);
        tx.focus("draft", false);
        self.send(Command::Select(conversation), tx);
        if self.connected() && self.summary(conversation).is_some_and(|chat| chat.unread) {
            self.send(Command::Update(conversation, ChatChange::Unread(false)), tx);
        }
    }

    /// Prepares a chat in the current project.
    pub fn create_here(&mut self, tx: &mut Tx) {
        self.create(self.project_id().map(str::to_owned), tx);
    }

    /// Prepares a chat; select a runner before committing to a session.
    pub fn create(&mut self, project: Option<String>, tx: &mut Tx) {
        self.prepare_chat(project, None, tx);
    }

    pub fn create_worktree(&mut self, project: String, worktree: Worktree, tx: &mut Tx) {
        self.prepare_chat(Some(project), Some(worktree), tx);
    }

    fn prepare_chat(&mut self, project: Option<String>, worktree: Option<Worktree>, tx: &mut Tx) {
        if !self.connected() || self.new_chat.is_some() {
            return;
        }
        self.save_draft();
        self.reset_thread();
        self.new_chat = Some(NewChat {
            project,
            worktree,
            request: None,
        });
        self.update_title(tx);
        tx.focus("draft", false);
        if self.harnesses.len() == 1 {
            self.bind_harness(None, tx);
        }
    }

    pub fn bind_harness(&mut self, harness: Option<String>, tx: &mut Tx) {
        let Some(chat) = self.new_chat.as_ref().filter(|chat| chat.request.is_none()) else {
            return;
        };
        let params = CreateConversation {
            project: chat.project.clone(),
            worktree: chat.worktree.clone(),
            harness,
            ..CreateConversation::default()
        };
        let request = self.next_create;
        self.next_create += 1;
        if self.send(Command::Create { request, params }, tx)
            && let Some(chat) = &mut self.new_chat
        {
            chat.request = Some(request);
        }
        self.open_picker = None;
    }

    pub fn created(&mut self, request: u64, conversation: Option<ConversationId>, tx: &mut Tx) {
        let Some(chat) = self
            .new_chat
            .as_mut()
            .filter(|chat| chat.request == Some(request))
        else {
            return;
        };
        chat.request = None;
        if let Some(id) = conversation {
            if let Some(project) = &chat.project {
                self.navigation.expand_project(project);
            }
            self.new_chat = None;
            self.select_bound(id, tx);
        }
    }

    fn select_bound(&mut self, id: ConversationId, tx: &mut Tx) {
        self.selected = Some(id);
        tx.focus("draft", false);
        self.send(Command::Select(id), tx);
    }

    pub fn project_id(&self) -> Option<&str> {
        if let Some(chat) = &self.new_chat {
            return chat.project.as_deref();
        }
        let id = self.selected?;
        self.summary(id).map_or_else(
            || {
                self.conversation
                    .as_ref()
                    .and_then(|s| s.project.as_deref())
            },
            |summary| summary.project.as_deref(),
        )
    }

    pub fn project(&self) -> Option<&Project> {
        let id = self.project_id()?;
        self.projects.iter().find(|project| project.id == id)
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.conversation.as_ref().map(|s| s.status),
            Some(ConversationStatus::Working | ConversationStatus::Approval)
        )
    }

    pub fn abort(&mut self, tx: &mut Tx) {
        if self.connected()
            && self.busy()
            && let Some(id) = self.selected
        {
            self.send(Command::Abort(id), tx);
        }
    }

    /// Tries to connect now instead of waiting for the next attempt.
    pub fn reconnect(&mut self, tx: &mut Tx) {
        self.status = Status::Connecting;
        self.update_title(tx);
        self.send(Command::Reconnect, tx);
    }

    pub fn can_attach(&self) -> bool {
        self.connected()
            && self
                .conversation
                .as_ref()
                .is_some_and(|state| !state.read_only)
            && self
                .selected
                .is_some_and(|id| !self.attachment_reads.contains(&id))
    }

    pub fn start_attachments(&mut self, source: Source, tx: &mut Tx) -> bool {
        if self.can_attach()
            && let Some(conversation) = self.selected
            && self.send(
                Command::Attach {
                    conversation,
                    source,
                },
                tx,
            )
        {
            self.attachment_reads.insert(conversation);
            return true;
        }
        false
    }

    pub fn loaded_attachments(&mut self, id: ConversationId, loaded: Loaded, tx: &mut Tx) {
        self.attachment_reads.remove(&id);
        let mut errors = loaded.errors;
        if !loaded.attachments.is_empty() {
            let existing = if self.selected == Some(id) {
                &mut self.attachments
            } else {
                &mut self.drafts.entry(id).or_default().attachments
            };
            let mut size = existing.iter().map(|file| file.data.len()).sum::<usize>();
            for file in loaded.attachments {
                if size + file.data.len() > attachments::ENCODED_LIMIT {
                    errors.push(format!(
                        "{}: attachments exceed the message limit",
                        file.name
                    ));
                } else {
                    size += file.data.len();
                    existing.push(file);
                }
            }
        }
        if !errors.is_empty() {
            self.toast(tx, "Couldn't attach files", Some(errors.join("\n")));
        }
    }

    pub fn can_send(&self) -> bool {
        self.can_attach() && (!self.draft.text.trim().is_empty() || !self.attachments.is_empty())
    }

    /// Sends the composer's text and files to the selected conversation.
    pub fn submit(&mut self, tx: &mut Tx) {
        if self.can_send()
            && let Some(conversation) = self.selected
        {
            let submit = Submit {
                conversation: Some(conversation),
                text: self.draft.text.clone(),
                attachments: mem::take(&mut self.attachments),
                ..Submit::default()
            };
            match self.commands.send(Command::Submit(submit)) {
                Ok(()) => {
                    self.draft.clear();
                    self.completion.reset();
                    tx.scroll_to_bottom("timeline");
                }
                Err(error) => {
                    if let Command::Submit(submit) = error.0 {
                        self.attachments = submit.attachments;
                    }
                    self.toast(tx, "Offline", None);
                }
            }
        }
    }

    pub fn restore_message(&mut self, mut submit: Submit) {
        let Some(conversation) = submit.conversation else {
            return;
        };
        let (draft, attachments) = if self.selected == Some(conversation) {
            (&mut self.draft, &mut self.attachments)
        } else {
            let draft = self.drafts.entry(conversation).or_default();
            (&mut draft.edit, &mut draft.attachments)
        };
        if !submit.text.is_empty() {
            if !draft.text.is_empty() {
                draft.set_caret(0);
                draft.insert("\n\n");
                draft.set_caret(0);
            }
            draft.insert(&submit.text);
        }
        submit.attachments.append(attachments);
        *attachments = submit.attachments;
    }

    /// The conversation as listed, once the list has it.
    pub fn summary(&self, conversation: ConversationId) -> Option<&ConversationSummary> {
        self.conversations
            .iter()
            .find(|summary| summary.id == conversation)
    }

    /// The conversation's name in the sidebar and header: the backend's title,
    /// or what it is until the backend has one.
    pub fn title(&self, conversation: ConversationId) -> &str {
        self.summary(conversation).map_or_else(
            || {
                if self.root == Some(conversation) {
                    "Orchestrator"
                } else {
                    "New chat"
                }
            },
            chat_title,
        )
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

pub fn chat_title(chat: &ConversationSummary) -> &str {
    chat.title.as_deref().unwrap_or(match chat.kind {
        ConversationKind::Orchestrator => "Orchestrator",
        ConversationKind::Chat => "New chat",
        ConversationKind::Subagent => "Subagent",
    })
}
