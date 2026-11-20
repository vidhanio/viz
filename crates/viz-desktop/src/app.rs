//! The window's state and what the user can do to it. The backend thread fills
//! it in through the app handle; the views only read it.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    mem,
    time::Duration,
};

use rune::{TextEdit, Tx};
use rune_kit::shell::PaneState;
use tokio::sync::mpsc;
use viz_client::protocol::{
    Attachment, Branches, ConversationId, ConversationKind, ConversationState, ConversationStatus,
    ConversationSummary, CreateConversation, DefaultWorkspace, Harness, ManagedWorktree,
    OptionValue, Project, Submit, Workspace,
};

use crate::{
    attachments::{self, Loaded, Source, Target as AttachmentTarget},
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
/// The window's size on launch, in points; headless renders use it too.
pub const WINDOW_SIZE: (f32, f32) = (1240.0, 760.0);

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

/// A local draft. No ACP session is open until its first send.
#[derive(Debug)]
pub struct NewChat {
    pub id: u64,
    pub project: Option<String>,
    pub workspace: Workspace,
    pub branches: Option<Branches>,
    pub worktrees: Vec<ManagedWorktree>,
    pub harness: Option<String>,
    pub options: BTreeMap<String, OptionValue>,
    pub sending: bool,
    loading_choices: bool,
}

impl NewChat {
    pub fn new(id: u64, project: Option<&Project>) -> Self {
        Self {
            id,
            project: project.map(|project| project.id.clone()),
            workspace: match project.map(|project| project.default_workspace) {
                Some(DefaultWorkspace::Worktree) => Workspace::Worktree { base: None },
                _ => Workspace::Local,
            },
            branches: None,
            worktrees: Vec::new(),
            harness: None,
            options: BTreeMap::new(),
            sending: false,
            loading_choices: false,
        }
    }

    pub fn active_harness<'a>(&self, runners: &'a [Harness]) -> Option<&'a Harness> {
        runners.iter().find(|runner| {
            self.harness
                .as_deref()
                .map_or(runner.default, |name| name == runner.name)
        })
    }
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
    next_draft: u64,
    /// Timeline rows the user opened (tool calls, thinking), by key.
    pub expanded: HashSet<String>,
    /// The composer's text.
    pub draft: TextEdit,
    pub attachments: Vec<Attachment>,
    attachment_reads: HashSet<AttachmentTarget>,
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
    /// The window's width in view points, as last drawn; layout that has to
    /// choose what fits reads it.
    pub window_width: f32,
    /// Chats left by navigating, newest last, for back and forward.
    back: Vec<ConversationId>,
    forward: Vec<ConversationId>,
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
            next_draft: 0,
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
            window_width: WINDOW_SIZE.0,
            back: Vec::new(),
            forward: Vec::new(),
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

    /// Takes a new conversation list. A turn that ended or failed where the
    /// user isn't looking becomes a desktop notification.
    pub fn set_conversations(&mut self, conversations: Vec<ConversationSummary>, tx: &mut Tx) {
        use ConversationStatus::{Failed, Idle, Working};
        for chat in &conversations {
            let Some(before) = self.summary(chat.id).map(|old| old.status) else {
                continue;
            };
            let ran = before == Working;
            let watched = self.focused && self.selected == Some(chat.id);
            let body = match chat.status {
                _ if before == chat.status || watched => continue,
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
        // The selected chat may have been deleted by any client. Use the old
        // list to find its nearest surviving parent before discarding it.
        let fallback = self
            .selected
            .and_then(|id| self.summary(id))
            .filter(|chat| !conversations.iter().any(|current| current.id == chat.id))
            .and_then(|old| {
                let mut ancestor = old.parent;
                while let Some(id) = ancestor {
                    if conversations.iter().any(|current| current.id == id) {
                        return Some(id);
                    }
                    ancestor = self.summary(id).and_then(|parent| parent.parent);
                }
                self.root
            });
        self.conversations = conversations;
        if let Some(fallback) = fallback
            && self.connected()
            && self.new_chat.is_none()
        {
            self.select(fallback, tx);
        }
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

    /// Opens `conversation` in the main pane, remembering where it came from
    /// for [`Desktop::go_back`].
    pub fn select(&mut self, conversation: ConversationId, tx: &mut Tx) {
        if let Some(current) = self.selected.filter(|current| *current != conversation) {
            self.back.push(current);
            self.forward.clear();
        }
        self.open(conversation, tx);
    }

    /// The chat open before this one.
    pub fn go_back(&mut self, tx: &mut Tx) {
        self.travel(true, tx);
    }

    /// The chat [`Desktop::go_back`] left.
    pub fn go_forward(&mut self, tx: &mut Tx) {
        self.travel(false, tx);
    }

    fn travel(&mut self, back: bool, tx: &mut Tx) {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        // Deleted chats drop out of the history as it's walked.
        let Some(next) = std::iter::from_fn(|| from.pop())
            .find(|id| self.conversations.iter().any(|chat| chat.id == *id))
        else {
            return;
        };
        if let Some(current) = self.selected {
            to.push(current);
        }
        self.open(next, tx);
    }

    fn open(&mut self, conversation: ConversationId, tx: &mut Tx) {
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
        let chat = NewChat::new(self.next_draft, self.project());
        self.prepare_chat(chat, tx);
    }

    /// Open a draft for a project without creating its backend session.
    pub fn create_in_project(&mut self, id: &str, tx: &mut Tx) {
        if let Some(project) = self.projects.iter().find(|project| project.id == id) {
            let chat = NewChat::new(self.next_draft, Some(project));
            self.prepare_chat(chat, tx);
        }
    }

    fn prepare_chat(&mut self, mut chat: NewChat, tx: &mut Tx) {
        if !self.connected() || self.new_chat.is_some() {
            return;
        }
        self.next_draft += 1;
        chat.loading_choices = self.send(
            Command::LoadDraft {
                id: chat.id,
                project: chat.project.clone(),
            },
            tx,
        );
        self.save_draft();
        self.reset_thread();
        self.new_chat = Some(chat);
        self.update_title(tx);
        tx.focus("draft", false);
    }

    pub fn set_draft_workspace(
        &mut self,
        id: u64,
        branches: Option<Branches>,
        worktrees: Vec<ManagedWorktree>,
    ) {
        if let Some(chat) = self.new_chat.as_mut().filter(|chat| chat.id == id) {
            chat.loading_choices = false;
            chat.branches = branches;
            chat.worktrees = worktrees;
        }
    }

    pub fn created(&mut self, draft: u64, conversation: ConversationId, tx: &mut Tx) {
        let Some(chat) = self.new_chat.as_ref().filter(|chat| chat.id == draft) else {
            return;
        };
        if let Some(project) = &chat.project {
            self.navigation.expand_project(project);
        }
        self.new_chat = None;
        self.selected = Some(conversation);
        tx.focus("draft", false);
        self.send(Command::Select(conversation), tx);
    }

    pub fn restore_first_message(&mut self, id: u64, submit: Submit) {
        if let Some(chat) = self.new_chat.as_mut().filter(|chat| chat.id == id) {
            chat.sending = false;
            restore_draft(&mut self.draft, &mut self.attachments, submit);
        }
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

    /// A managed checkout used by the selected conversation.
    pub fn worktree(&self) -> Option<(&str, Option<&str>)> {
        let git = self
            .conversation
            .as_ref()
            .and_then(|state| state.git.as_ref());
        if let Some((git, path)) =
            git.and_then(|git| git.worktree.as_deref().map(|path| (git, path)))
        {
            return Some((path, git.branch.as_deref()));
        }
        let summary = self.summary(self.selected?)?;
        Some((summary.worktree.as_deref()?, summary.branch.as_deref()))
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.conversation.as_ref().map(|s| s.status),
            Some(ConversationStatus::Working)
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

    fn attachment_target(&self) -> Option<AttachmentTarget> {
        if !self.connected() {
            return None;
        }
        if let Some(chat) = &self.new_chat {
            return (!chat.sending).then_some(AttachmentTarget::Draft(chat.id));
        }
        self.conversation.as_ref()?;
        self.selected.map(AttachmentTarget::Conversation)
    }

    pub fn can_attach(&self) -> bool {
        self.attachment_target()
            .is_some_and(|target| !self.attachment_reads.contains(&target))
    }

    pub fn start_attachments(&mut self, source: Source, tx: &mut Tx) -> bool {
        if let Some(target) = self
            .attachment_target()
            .filter(|target| !self.attachment_reads.contains(target))
            && self.send(Command::Attach { target, source }, tx)
        {
            self.attachment_reads.insert(target);
            return true;
        }
        false
    }

    pub fn loaded_attachments(&mut self, target: AttachmentTarget, loaded: Loaded, tx: &mut Tx) {
        self.attachment_reads.remove(&target);
        let mut errors = loaded.errors;
        if !loaded.attachments.is_empty() {
            let existing = match target {
                AttachmentTarget::Draft(id) => {
                    if self.new_chat.as_ref().is_none_or(|chat| chat.id != id) {
                        return;
                    }
                    &mut self.attachments
                }
                AttachmentTarget::Conversation(id) if self.selected == Some(id) => {
                    &mut self.attachments
                }
                AttachmentTarget::Conversation(id) => {
                    &mut self.drafts.entry(id).or_default().attachments
                }
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
        self.can_attach()
            && self.new_chat.as_ref().is_none_or(|chat| {
                !chat.loading_choices && chat.active_harness(&self.harnesses).is_some()
            })
            && (!self.draft.text.trim().is_empty() || !self.attachments.is_empty())
    }

    /// Creates a draft's session on first send, or sends to an existing chat.
    pub fn submit(&mut self, tx: &mut Tx) {
        if !self.can_send() {
            return;
        }
        let submit = Submit {
            conversation: self.selected,
            text: self.draft.text.clone(),
            attachments: mem::take(&mut self.attachments),
            ..Submit::default()
        };
        let command = if let Some(chat) = &mut self.new_chat {
            let workspace = chat.project.as_ref().map(|_| chat.workspace.clone());
            chat.sending = true;
            Command::Create {
                draft: chat.id,
                params: CreateConversation {
                    project: chat.project.clone(),
                    workspace,
                    harness: chat.harness.clone(),
                    options: chat.options.clone(),
                },
                submit,
            }
        } else {
            Command::Submit(submit)
        };
        match self.commands.send(command) {
            Ok(()) => {
                self.draft.clear();
                self.completion.reset();
                self.open_picker = None;
                tx.scroll_to_bottom("timeline");
            }
            Err(error) => {
                match error.0 {
                    Command::Create { draft, submit, .. } => {
                        if let Some(chat) = self.new_chat.as_mut().filter(|chat| chat.id == draft) {
                            chat.sending = false;
                        }
                        self.attachments = submit.attachments;
                    }
                    Command::Submit(submit) => self.attachments = submit.attachments,
                    _ => unreachable!("only inputs are sent by the composer"),
                }
                self.toast(tx, "Offline", None);
            }
        }
    }

    pub fn restore_message(&mut self, submit: Submit) {
        let Some(conversation) = submit.conversation else {
            return;
        };
        let (draft, attachments) = if self.selected == Some(conversation) {
            (&mut self.draft, &mut self.attachments)
        } else {
            let draft = self.drafts.entry(conversation).or_default();
            (&mut draft.edit, &mut draft.attachments)
        };
        restore_draft(draft, attachments, submit);
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

    /// A toast that stays, for a preview scene (it has no `Tx` clock).
    pub fn preview_toast(&mut self, title: &str, detail: &str) {
        self.toasts.push(Toast {
            id: self.next_toast,
            title: title.into(),
            detail: Some(detail.into()),
            due: f64::INFINITY,
        });
        self.next_toast += 1;
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

fn restore_draft(draft: &mut TextEdit, attachments: &mut Vec<Attachment>, mut submit: Submit) {
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

pub fn chat_title(chat: &ConversationSummary) -> &str {
    if !chat.title.is_empty() {
        return &chat.title;
    }
    match chat.kind {
        ConversationKind::Orchestrator => "Orchestrator",
        ConversationKind::Chat => "New chat",
        ConversationKind::Subagent => "Subagent",
    }
}
