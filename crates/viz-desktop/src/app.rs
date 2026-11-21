//! The window's state and what the user can do to it. The server thread fills
//! it in through the app handle; the views only read it.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    mem,
    time::Duration,
};

use rune::{Spring, SpringPair, TextEdit, Transition, Tx};
use rune_kit::shell::PaneState;
use tokio::sync::mpsc;
use viz_client::protocol::{
    Attachment, ConfigOption, Environment, Harness, LaunchThread, OptionValue, Project, SendInput,
    Task, ThreadId, ThreadState, ThreadStatus, ThreadSummary,
};

use crate::{
    attachments::{self, Loaded, Source, Target as AttachmentTarget},
    composer::{Completion, ModelMenu},
    palette::Palette,
    server::{Command, ThreadChange},
    sidebar::Navigation,
    thread::Thread,
};

/// How long a toast stays up.
const TOAST_LIFE: Duration = Duration::from_secs(6);
/// The most toasts shown at once; older ones go first.
const TOAST_MAX: usize = 3;
/// The window's size on launch, in points; headless renders use it too.
pub const WINDOW_SIZE: (f32, f32) = (1240.0, 760.0);

/// The server connection, as the sidebar footer shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Trying to connect.
    Connecting,
    /// Connected to a server of this version.
    Connected {
        /// Server version.
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
pub struct NewThread {
    pub id: u64,
    pub project: Option<String>,
    pub harness: Option<String>,
    pub options: BTreeMap<String, OptionValue>,
    pub sending: bool,
    loading_choices: bool,
}

impl NewThread {
    pub fn new(id: u64, project: Option<&Project>) -> Self {
        Self {
            id,
            project: project.map(|project| project.path.clone()),
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
                .map_or(runner.default, |id| id == runner.id)
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

/// How far the transcript travels when the open thread changes.
pub const PANE_TRAVEL: f32 = crate::theme::S40;
/// How far the toolbar title travels with it, inside its own clip: tall enough
/// to clear one title line without overlapping the outgoing title.
pub const TITLE_TRAVEL: f32 = crate::theme::S24;
/// Delay before a new toolbar title starts fading in so the outgoing title
/// clears first instead of colliding mid-slide.
pub const TITLE_ENTER_DELAY: f32 = 0.08;
/// The switch's springs: short and calm, without bounce.
const SWITCH_SPRINGS: SpringPair = SpringPair::new(Spring::new(0.24, 0.0), Spring::new(0.18, 0.0));
/// Faster exit on the toolbar title so old and new text never crossfade on top
/// of one another.
const TITLE_SPRINGS: SpringPair = SpringPair::new(Spring::new(0.20, 0.0), Spring::new(0.10, 0.0));

/// Which way the main pane moves when the open thread changes, decided by
/// [`Desktop::show`] from where the two threads sit. Threads further down
/// the sidebar are below: going down moves the content up. A delegated
/// task's thread is to the right of the thread that delegated it. The
/// composer and task panel stay put; Reduce Motion turns travel into a fade.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Switch {
    /// No motion: a draft becoming its thread, another environment.
    #[default]
    Cut,
    /// A move with no direction: a draft, a thread the filter hides.
    Fade,
    /// To a thread listed further down: content moves up.
    Down,
    /// To a thread listed further up: content moves down.
    Up,
    /// Into a delegated task's thread: content moves left.
    Into,
    /// Back out to the thread that delegated it: content moves right.
    Out,
}

impl Switch {
    /// Where the new pane enters from, as a unit vector.
    const fn from(self) -> (f32, f32) {
        match self {
            Self::Cut | Self::Fade => (0.0, 0.0),
            Self::Down => (0.0, 1.0),
            Self::Up => (0.0, -1.0),
            Self::Into => (1.0, 0.0),
            Self::Out => (-1.0, 0.0),
        }
    }

    /// The new pane's entrance, travelling `travel` points.
    pub fn enter(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.motion(x * travel, y * travel, SWITCH_SPRINGS)
    }

    /// The old pane's exit, for `El::child_exit`: away the way the new one
    /// came.
    pub fn exit(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.motion(-x * travel, -y * travel, SWITCH_SPRINGS)
    }

    /// The new toolbar title's entrance, staggered behind the outgoing title.
    pub fn title_enter(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.motion(x * travel, y * travel, TITLE_SPRINGS)
    }

    /// The old toolbar title's fast exit so old and new text do not overlap.
    pub fn title_exit(self, travel: f32) -> Transition {
        let (x, y) = self.from();
        self.motion(-x * travel, -y * travel, TITLE_SPRINGS)
    }

    fn motion(self, x: f32, y: f32, springs: SpringPair) -> Transition {
        if self == Self::Cut {
            return Transition::none();
        }
        Transition::offset(x, y)
            .combined(Transition::opacity())
            .springs(springs)
    }
}

/// The window's state.
#[derive(Debug)]
pub struct Desktop {
    commands: mpsc::UnboundedSender<Command>,
    /// The connection.
    pub status: Status,
    /// Where the server is, once the config is read.
    pub endpoint: Option<String>,
    pub environment: Option<String>,
    pub environments: Vec<Environment>,
    pub tasks: Vec<Task>,
    /// Every thread as the server lists it, titles included.
    pub threads: Vec<ThreadSummary>,
    pub projects: Vec<Project>,
    pub harnesses: Vec<Harness>,
    /// The thread in the main pane.
    pub selected: Option<ThreadId>,
    /// Its transcript and live state, once its view arrives.
    pub thread: Thread,
    /// The selected thread's transcript hasn't arrived since it was shown.
    pub loading: bool,
    pub history_loading: bool,
    pub state: Option<ThreadState>,
    pub new_thread: Option<NewThread>,
    /// How the last change of thread moves the main pane.
    pub switch: Switch,
    next_draft: u64,
    /// Timeline rows the user opened (tool calls, thinking), by key.
    pub expanded: HashSet<String>,
    /// The composer's text.
    pub draft: TextEdit,
    pub attachments: Vec<Attachment>,
    attachment_reads: HashSet<AttachmentTarget>,
    environment_epoch: u64,
    drafts: HashMap<ThreadId, Draft>,
    inactive_drafts: HashMap<String, HashMap<ThreadId, Draft>>,
    pub completion: Completion,
    pub model_menu: ModelMenu,
    pub open_picker: Option<String>,
    pub navigation: Navigation,
    /// The sidebar's width and whether it's open.
    pub sidebar: PaneState,
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
    pub tool_clock: i64,
    /// Threads left by navigating, newest last, for back and forward.
    back: Vec<ThreadId>,
    forward: Vec<ThreadId>,
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
            environment: None,
            environments: Vec::new(),
            tasks: Vec::new(),
            threads: Vec::new(),
            projects: Vec::new(),
            harnesses: Vec::new(),
            selected: None,
            thread: Thread::default(),
            loading: false,
            history_loading: false,
            state: None,
            new_thread: None,
            switch: Switch::Cut,
            next_draft: 0,
            expanded: HashSet::new(),
            draft,
            attachments: Vec::new(),
            attachment_reads: HashSet::new(),
            environment_epoch: 0,
            drafts: HashMap::new(),
            inactive_drafts: HashMap::new(),
            completion: Completion::default(),
            model_menu: ModelMenu::default(),
            open_picker: None,
            navigation: Navigation::default(),
            sidebar: PaneState::new(256.0).range(220.0, 384.0),
            palette: Palette::default(),
            toasts: Vec::new(),
            next_toast: 0,
            focused: true,
            window_width: WINDOW_SIZE.0,
            tool_clock: crate::theme::now_ms(),
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// A state with no connection thread behind it, for `AppSpec::init`;
    /// `main` replaces it.
    pub fn detached() -> Self {
        Self::new(mpsc::unbounded_channel().0)
    }

    /// Connected to a server.
    pub const fn connected(&self) -> bool {
        matches!(self.status, Status::Connected { .. })
    }

    /// Takes a new thread list. A turn that ended or failed where the
    /// user isn't looking becomes a desktop notification.
    pub fn set_threads(&mut self, threads: Vec<ThreadSummary>, tx: &mut Tx) {
        use ThreadStatus::{Failed, Idle, Working};
        for thread in &threads {
            let Some(before) = self.summary(thread.id).map(|old| old.status) else {
                continue;
            };
            let ran = before == Working;
            let watched = self.focused && self.selected == Some(thread.id);
            let body = match thread.status {
                _ if before == thread.status || watched => continue,
                _ if thread.parent.is_some() => continue,
                Idle if ran => "Finished",
                Failed if ran => "Failed",
                _ => continue,
            };
            self.send(
                Command::Notify {
                    title: thread_title(thread).to_owned(),
                    body,
                },
                tx,
            );
        }
        let fallback = self
            .selected
            .and_then(|id| self.summary(id))
            .filter(|thread| !threads.iter().any(|current| current.id == thread.id))
            .and_then(|old| {
                let mut ancestor = old.parent.filter(|_| {
                    old.parent_environment
                        .as_ref()
                        .is_none_or(|env| Some(env) == self.environment.as_ref())
                });
                while let Some(id) = ancestor {
                    if threads.iter().any(|current| current.id == id) {
                        return Some(id);
                    }
                    ancestor = self.summary(id).and_then(|parent| parent.parent);
                }
                None
            });
        self.threads = threads;
        if self.connected()
            && self.new_thread.is_none()
            && self.selected.is_some_and(|id| self.summary(id).is_none())
        {
            if let Some(id) = fallback {
                self.select(id, tx);
            } else {
                self.clear_selection();
                self.send(Command::ClearSelection, tx);
            }
        }
    }

    /// The window gained or lost focus; coming back reads the open thread.
    pub fn set_focus(&mut self, focused: bool, tx: &mut Tx) {
        self.focused = focused;
        if focused
            && self.connected()
            && let Some(id) = self
                .selected
                .filter(|&id| self.summary(id).is_some_and(|thread| thread.unread))
        {
            self.send(Command::Update(id, ThreadChange::Unread(false)), tx);
        }
    }

    /// Shows `thread`, loading until its view arrives.
    pub fn show(&mut self, thread: ThreadId) {
        if self.selected != Some(thread) || self.new_thread.is_some() {
            self.switch = if self.new_thread.is_some() {
                Switch::Fade
            } else {
                self.selected
                    .map_or(Switch::Cut, |from| self.direction(from, thread))
            };
            self.save_draft();
            self.new_thread = None;
            self.selected = Some(thread);
            let draft = self.drafts.remove(&thread).unwrap_or_default();
            self.draft = draft.edit;
            self.attachments = draft.attachments;
            self.reset_thread();
            self.loading = true;
            self.navigation
                .reveal(thread, &self.threads, self.environment.as_deref());
        }
    }

    /// Which way the pane moves from `from` to `to`.
    fn direction(&self, from: ThreadId, to: ThreadId) -> Switch {
        if self.descends(to, from) {
            return Switch::Into;
        }
        if self.descends(from, to) {
            return Switch::Out;
        }
        match (
            crate::sidebar::position(self, from),
            crate::sidebar::position(self, to),
        ) {
            (Some(a), Some(b)) if b > a => Switch::Down,
            (Some(a), Some(b)) if b < a => Switch::Up,
            _ => Switch::Fade,
        }
    }

    /// `thread` was delegated, directly or through other tasks, by
    /// `ancestor`.
    fn descends(&self, thread: ThreadId, ancestor: ThreadId) -> bool {
        let mut seen = 0;
        let mut at = self.summary(thread).and_then(|summary| summary.parent);
        while let Some(parent) = at {
            if parent == ancestor {
                return true;
            }
            seen += 1;
            if seen > self.threads.len() {
                return false;
            }
            at = self.summary(parent).and_then(|summary| summary.parent);
        }
        false
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

    pub fn clear_selection(&mut self) {
        self.save_draft();
        self.reset_thread();
    }

    fn reset_thread(&mut self) {
        self.thread = Thread::default();
        self.loading = false;
        self.history_loading = false;
        self.state = None;
        self.expanded.clear();
        self.completion.reset();
        self.open_picker = None;
    }

    /// Applies a reset or appends committed ACP activity to the current
    /// viewport.
    pub fn apply_view(
        &mut self,
        id: ThreadId,
        entries: &[viz_client::protocol::Entry],
        reset: bool,
        before: Option<viz_client::protocol::EntryId>,
    ) {
        if self.selected != Some(id) {
            return;
        }
        if reset {
            self.thread = Thread::from_entries(entries, before);
            self.loading = false;
            self.history_loading = false;
        } else {
            self.thread.append(entries);
        }
        self.thread
            .sync_acp(self.state.as_ref().and_then(|state| state.live.as_ref()));
    }

    pub fn set_state(&mut self, id: ThreadId, state: ThreadState, tx: &mut Tx) {
        if self.selected != Some(id) {
            return;
        }
        let status_changed = self.state.as_ref().map(|prev| prev.status) != Some(state.status);
        self.thread.sync_acp(state.live.as_ref());
        self.state = Some(state);
        if status_changed {
            self.update_title(tx);
        }
    }

    pub fn update_title(&self, tx: &mut Tx) {
        tx.set_title(
            match (&self.status, self.state.as_ref().map(|s| s.status)) {
                (Status::Error(_), _) => "viz — Offline",
                (Status::Connecting, _) => "viz — Connecting",
                (_, Some(ThreadStatus::Working)) => "viz — Working",
                (_, Some(ThreadStatus::Failed)) => "viz — Failed",
                _ => "viz",
            },
        );
    }

    pub fn load_earlier(&mut self, tx: &mut Tx) {
        if !self.history_loading
            && self.connected()
            && let (Some(thread), Some(before)) = (self.selected, self.thread.before)
            && self.send(Command::LoadEarlier { thread, before }, tx)
        {
            self.history_loading = true;
        }
    }

    pub fn loaded_earlier(&mut self, id: ThreadId, thread: Option<Thread>, tx: &mut Tx) {
        if self.selected != Some(id) {
            return;
        }
        self.history_loading = false;
        if let Some(thread) = thread {
            if let Some(first) = self.thread.items.first() {
                tx.keep_on_screen(first.key());
            }
            self.thread = thread;
            self.thread
                .sync_acp(self.state.as_ref().and_then(|state| state.live.as_ref()));
        }
    }

    pub fn send(&mut self, command: Command, tx: &mut Tx) -> bool {
        if self.commands.send(command).is_ok() {
            return true;
        }
        self.toast(tx, "Offline", None);
        false
    }

    /// Opens `thread` in the main pane, remembering where it came from
    /// for [`Desktop::go_back`].
    pub fn select(&mut self, thread: ThreadId, tx: &mut Tx) {
        if let Some(current) = self.selected.filter(|current| *current != thread) {
            self.back.push(current);
            self.forward.clear();
        }
        self.open(thread, tx);
    }

    /// The thread open before this one.
    pub fn go_back(&mut self, tx: &mut Tx) {
        self.travel(true, tx);
    }

    /// The thread [`Desktop::go_back`] left.
    pub fn go_forward(&mut self, tx: &mut Tx) {
        self.travel(false, tx);
    }

    fn travel(&mut self, back: bool, tx: &mut Tx) {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        let Some(next) = std::iter::from_fn(|| from.pop())
            .find(|id| self.threads.iter().any(|thread| thread.id == *id))
        else {
            return;
        };
        if let Some(current) = self.selected {
            to.push(current);
        }
        self.open(next, tx);
    }

    fn open(&mut self, thread: ThreadId, tx: &mut Tx) {
        self.show(thread);
        self.update_title(tx);
        tx.focus("draft", false);
        self.send(Command::Select(thread), tx);
        if self.connected() && self.summary(thread).is_some_and(|thread| thread.unread) {
            self.send(Command::Update(thread, ThreadChange::Unread(false)), tx);
        }
    }

    /// Prepares a draft in the open thread’s project, if any.
    pub fn create_here(&mut self, tx: &mut Tx) {
        let thread = NewThread::new(self.next_draft, self.project());
        self.prepare_thread(thread, tx);
    }

    /// Open a draft for a project without creating its server session.
    pub fn create_in_project(&mut self, id: &str, tx: &mut Tx) {
        if let Some(project) = self.projects.iter().find(|project| project.path == id) {
            let thread = NewThread::new(self.next_draft, Some(project));
            self.prepare_thread(thread, tx);
        }
    }

    /// The server added `project`. Listed at once (the list refresh may come
    /// later), and an open draft takes it: a folder added while drafting is
    /// the one the thread is meant for, whether it came from the draft's own
    /// add button, the sidebar or the palette.
    pub fn project_added(&mut self, project: Project) {
        if let Some(thread) = &mut self.new_thread
            && !thread.sending
        {
            thread.project = Some(project.path.clone());
        }
        if !self.projects.iter().any(|known| known.path == project.path) {
            let at = self.projects.partition_point(|known| {
                (&known.name, &known.path) < (&project.name, &project.path)
            });
            self.projects.insert(at, project);
        }
    }

    fn prepare_thread(&mut self, mut thread: NewThread, tx: &mut Tx) {
        if !self.connected() || self.new_thread.is_some() {
            return;
        }
        self.next_draft += 1;
        thread.loading_choices = self.send(Command::LoadDraft(thread.id), tx);
        self.switch = Switch::Fade;
        self.clear_selection();
        self.send(Command::ClearSelection, tx);
        self.new_thread = Some(thread);
        self.update_title(tx);
        tx.focus("draft", false);
    }

    /// The draft's runner choices arrived (or failed to).
    pub fn draft_loaded(&mut self, id: u64) {
        if let Some(thread) = self.new_thread.as_mut().filter(|thread| thread.id == id) {
            thread.loading_choices = false;
        }
    }

    /// Takes a runner's options with another model selected
    /// (`harnesses.options`): its catalog, now with that model's thinking
    /// levels.
    pub fn set_harness_options(&mut self, harness: &str, options: Vec<ConfigOption>) {
        if let Some(runner) = self
            .harnesses
            .iter_mut()
            .find(|runner| runner.id == harness)
        {
            runner.options = options;
        }
    }

    pub fn created(&mut self, draft: u64, id: ThreadId, tx: &mut Tx) {
        let Some(thread) = self.new_thread.as_ref().filter(|thread| thread.id == draft) else {
            return;
        };
        if let Some(project) = &thread.project {
            self.navigation.expand_project(project);
        }
        self.new_thread = None;
        self.selected = Some(id);
        self.switch = Switch::Cut;
        self.loading = true;
        tx.focus("draft", false);
        self.send(Command::Select(id), tx);
    }

    pub fn restore_first_message(&mut self, id: u64, params: LaunchThread) {
        if let Some(thread) = self.new_thread.as_mut().filter(|thread| thread.id == id) {
            thread.sending = false;
            restore_draft(
                &mut self.draft,
                &mut self.attachments,
                params.text.as_deref().unwrap_or_default(),
                params.attachments,
            );
        }
    }

    pub fn project_path(&self) -> Option<&str> {
        if let Some(thread) = &self.new_thread {
            return thread.project.as_deref();
        }
        let id = self.selected?;
        self.summary(id).map_or_else(
            || self.state.as_ref().and_then(|s| s.project.as_deref()),
            |summary| summary.project.as_deref(),
        )
    }

    pub fn project(&self) -> Option<&Project> {
        let id = self.project_path()?;
        self.projects.iter().find(|project| project.path == id)
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.state.as_ref().map(|s| s.status),
            Some(ThreadStatus::Working)
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

    /// Selects another environment and optionally opens a thread there.
    pub fn open_in_environment(
        &mut self,
        environment: String,
        thread: Option<ThreadId>,
        tx: &mut Tx,
    ) {
        if self.environment.as_deref() == Some(&environment) {
            if let Some(thread) = thread {
                self.select(thread, tx);
            }
            return;
        }
        if !self.send(
            Command::Environment {
                environment: environment.clone(),
                thread,
            },
            tx,
        ) {
            return;
        }
        self.save_draft();
        if let Some(previous) = self.environment.take()
            && !self.drafts.is_empty()
        {
            self.inactive_drafts
                .insert(previous, mem::take(&mut self.drafts));
        }
        self.drafts = self
            .inactive_drafts
            .remove(&environment)
            .unwrap_or_default();
        self.status = Status::Connecting;
        self.environment = Some(environment);
        self.new_thread = None;
        self.switch = Switch::Cut;
        self.draft.clear();
        self.attachments.clear();
        self.attachment_reads.clear();
        self.environment_epoch = self.environment_epoch.wrapping_add(1);
        self.back.clear();
        self.forward.clear();
        self.threads.clear();
        self.projects.clear();
        self.harnesses.clear();
        self.tasks.clear();
        self.navigation.dismiss_overlays();
        self.palette = Palette::default();
        self.reset_thread();
        self.update_title(tx);
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
        if let Some(thread) = &self.new_thread {
            return (!thread.sending).then_some(AttachmentTarget::Draft(thread.id));
        }
        self.state.as_ref()?;
        self.selected.map(AttachmentTarget::Thread)
    }

    pub fn can_attach(&self) -> bool {
        self.attachment_target()
            .is_some_and(|target| !self.attachment_reads.contains(&target))
    }

    pub fn start_attachments(&mut self, source: Source, tx: &mut Tx) -> bool {
        if let Some(target) = self
            .attachment_target()
            .filter(|target| !self.attachment_reads.contains(target))
            && self.send(
                Command::Attach {
                    scope: self.environment_epoch,
                    target,
                    source,
                },
                tx,
            )
        {
            self.attachment_reads.insert(target);
            return true;
        }
        false
    }

    pub fn loaded_attachments(
        &mut self,
        scope: u64,
        target: AttachmentTarget,
        loaded: Loaded,
        tx: &mut Tx,
    ) {
        if scope != self.environment_epoch || !self.attachment_reads.remove(&target) {
            return;
        }
        let mut errors = loaded.errors;
        if !loaded.attachments.is_empty() {
            let existing = match target {
                AttachmentTarget::Draft(id) => {
                    if self
                        .new_thread
                        .as_ref()
                        .is_none_or(|thread| thread.id != id)
                    {
                        return;
                    }
                    &mut self.attachments
                }
                AttachmentTarget::Thread(id) if self.selected == Some(id) => &mut self.attachments,
                AttachmentTarget::Thread(id) => &mut self.drafts.entry(id).or_default().attachments,
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
            self.toast(tx, "Couldn’t attach files", Some(errors.join("\n")));
        }
    }

    pub fn can_send(&self) -> bool {
        self.can_attach()
            && self.new_thread.as_ref().is_none_or(|thread| {
                !thread.loading_choices && thread.active_harness(&self.harnesses).is_some()
            })
            && (!self.draft.text.trim().is_empty() || !self.attachments.is_empty())
    }

    /// Creates a draft's session on first send, or sends to an existing thread.
    pub fn send_input(&mut self, tx: &mut Tx) {
        if !self.can_send() {
            return;
        }
        let command = if let Some(thread) = &mut self.new_thread {
            thread.sending = true;
            Command::Create {
                draft: thread.id,
                params: LaunchThread {
                    project: thread.project.clone(),
                    harness: thread.harness.clone(),
                    options: thread.options.clone(),
                    text: Some(self.draft.text.clone()),
                    attachments: mem::take(&mut self.attachments),
                    ..LaunchThread::default()
                },
            }
        } else if let Some(thread) = self.selected {
            Command::SendInput(SendInput {
                thread,
                text: self.draft.text.clone(),
                attachments: mem::take(&mut self.attachments),
                request_id: None,
                origin: None,
                mode: None,
            })
        } else {
            return;
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
                    Command::Create { draft, params } => {
                        if let Some(thread) =
                            self.new_thread.as_mut().filter(|thread| thread.id == draft)
                        {
                            thread.sending = false;
                        }
                        self.attachments = params.attachments;
                    }
                    Command::SendInput(send_input) => self.attachments = send_input.attachments,
                    _ => unreachable!("only inputs are sent by the composer"),
                }
                self.toast(tx, "Offline", None);
            }
        }
    }

    pub fn restore_message(&mut self, send_input: SendInput) {
        let thread = send_input.thread;
        let (draft, attachments) = if self.selected == Some(thread) {
            (&mut self.draft, &mut self.attachments)
        } else {
            let draft = self.drafts.entry(thread).or_default();
            (&mut draft.edit, &mut draft.attachments)
        };
        restore_draft(draft, attachments, &send_input.text, send_input.attachments);
    }

    /// The thread as listed, once the list has it.
    pub fn summary(&self, thread: ThreadId) -> Option<&ThreadSummary> {
        self.threads.iter().find(|summary| summary.id == thread)
    }

    /// The thread's name in the sidebar and header: the server's title,
    /// or what it is until the server has one.
    pub fn title(&self, thread: ThreadId) -> &str {
        self.summary(thread).map_or("New thread", thread_title)
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

fn restore_draft(
    draft: &mut TextEdit,
    attachments: &mut Vec<Attachment>,
    text: &str,
    mut files: Vec<Attachment>,
) {
    if !text.is_empty() {
        if !draft.text.is_empty() {
            draft.set_caret(0);
            draft.insert("\n\n");
            draft.set_caret(0);
        }
        draft.insert(text);
    }
    files.append(attachments);
    *attachments = files;
}

pub fn thread_title(thread: &ThreadSummary) -> &str {
    if !thread.title.is_empty() {
        return &thread.title;
    }
    "New thread"
}
