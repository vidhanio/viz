//! Imperative shell wrapper around [`Model`]: owns Rune widget state, exposes
//! effect sinks, and funnels every UI event through [`Desktop::dispatch`].

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rune::{ImageCache, TextEdit, Tx, image::Image};
use rune_kit::{ToastEffect, ToastEvent, ToastPage, ToastStack, shell::PaneState};
use tokio::sync::mpsc;

use crate::{
    effects::{self, ServerEffect},
    model::{
        DIFF_DEFAULT_WIDTH, DIFF_MAX_WIDTH, DIFF_MIN_WIDTH, DraftCursor, Model, PaneSwitch,
        SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, UndoTriage,
    },
    motion::Switch,
    motion_scene::MotionDemo,
    theme,
    ui::Tone,
    update::{self, Msg, ShellMsg},
};

/// How long a toast stays up while unpaused.
const TOAST_LIFE: Duration = Duration::from_secs(6);

/// Content carried by an entry in [`ShellWidgets::toasts`].
#[derive(Debug, Clone)]
pub struct ToastContent {
    /// One line.
    pub title: String,
    /// More, if there is any.
    pub detail: Option<String>,
    /// Semantic tone dot color for the toast card.
    pub tone: Tone,
    /// Optional undo payload rendered as an inline `Undo` button.
    pub undo: Option<UndoTriage>,
}

/// Stateful Rune and `rune-kit` widget controllers kept outside the pure
/// [`Model`].
pub struct ShellWidgets {
    pub terminal: crate::terminal::panel::Widgets,
    pub request_edit: TextEdit,
    pub request_step: Option<(viz_client::protocol::RequestId, usize)>,
    pub git_query: TextEdit,
    pub git_message: TextEdit,
    pub git_body: TextEdit,
    pub git_revision: TextEdit,
    pub git_run: TextEdit,
    pub git_diff: rune_kit::diff::DiffState,
    pub git_diff_tree: rune_kit::TreeModel<rune_kit::FileNode<(usize, u32, u32)>>,
    pub git_diff_source: Option<std::sync::Arc<viz_client::protocol::GitDiff>>,
    pub git_tree_open: bool,
    /// Main composer text editor.
    pub draft: TextEdit,
    /// Project filter combobox search input.
    pub project_filter_query: TextEdit,
    /// Sidebar inline thread title rename input.
    pub inline_rename: TextEdit,
    /// Sidebar rename dialog input.
    pub sidebar_dialog: TextEdit,
    /// Command palette query input.
    pub palette_query: TextEdit,
    /// Model picker search input.
    pub model_query: TextEdit,
    /// Resizable, collapsible sidebar pane state.
    pub sidebar_pane: PaneState,
    /// Resizable, collapsible docked diff inspector pane state.
    pub diff_pane: PaneState,
    /// Coalesced toast stack with hover and hidden-window timer pause.
    pub toasts: ToastStack<ToastContent>,
    /// Off-thread image decoder cache initialized in `AppSpec::setup`.
    pub images: Option<ImageCache>,
    /// Decoded project icon images keyed by `"{env}\0{project_path}"`.
    pub project_icons: HashMap<String, Image>,
}

impl std::fmt::Debug for ShellWidgets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellWidgets")
            .field("sidebar_pane", &self.sidebar_pane)
            .field("diff_pane", &self.diff_pane)
            .field("images_ready", &self.images.is_some())
            .field("project_icons", &self.project_icons.len())
            .finish_non_exhaustive()
    }
}

impl Default for ShellWidgets {
    fn default() -> Self {
        let mut draft = TextEdit::new("");
        draft.focused = true;
        Self {
            terminal: crate::terminal::panel::Widgets::default(),
            request_edit: TextEdit::new(""),
            request_step: None,
            git_query: TextEdit::new(""),
            git_message: TextEdit::new(""),
            git_body: TextEdit::new(""),
            git_revision: TextEdit::new(""),
            git_run: TextEdit::new(""),
            git_diff: rune_kit::diff::DiffState::new(Vec::new()),
            git_diff_tree: rune_kit::TreeModel::new(),
            git_diff_source: None,
            git_tree_open: false,
            draft,
            project_filter_query: TextEdit::new(""),
            inline_rename: TextEdit::new(""),
            sidebar_dialog: TextEdit::new(""),
            palette_query: TextEdit::new(""),
            model_query: TextEdit::new(""),
            sidebar_pane: PaneState::new(SIDEBAR_DEFAULT_WIDTH)
                .range(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            diff_pane: PaneState::new(DIFF_DEFAULT_WIDTH)
                .range(DIFF_MIN_WIDTH, DIFF_MAX_WIDTH)
                .collapsed(),
            toasts: ToastStack::default(),
            images: None,
            project_icons: HashMap::new(),
        }
    }
}

/// Root Rune application state: pure [`Model`] plus shell widgets and effect
/// sinks.
pub struct Desktop {
    /// Pure domain and client state.
    pub model: Model,
    /// Rune widget controllers synced with [`Self::model`].
    pub widgets: ShellWidgets,
    commands: mpsc::UnboundedSender<ServerEffect>,
    prefs_path: Option<PathBuf>,
    /// Scene-only override for Reduce Motion in headless verification.
    pub reduced_motion: Option<bool>,
    /// Interactive motion workbench or component gallery scene state.
    pub motion_demo: Option<MotionDemo>,
}

impl Desktop {
    /// Constructs a live desktop shell sending server effects to `commands`.
    #[must_use]
    pub fn new(commands: mpsc::UnboundedSender<ServerEffect>) -> Self {
        Self {
            model: Model::new(theme::now_ms()),
            widgets: ShellWidgets::default(),
            commands,
            prefs_path: None,
            reduced_motion: None,
            motion_demo: None,
        }
    }

    /// A detached shell with no server thread behind it, for `AppSpec::init`
    /// and headless scenes.
    #[must_use]
    pub fn detached() -> Self {
        Self::new(mpsc::unbounded_channel().0)
    }

    /// Loads persisted preferences from `$XDG_STATE_HOME/viz/desktop.json` and
    /// enables saving future changes there.
    pub fn restore_prefs(&mut self) {
        let Some(path) = effects::prefs::default_path() else {
            return;
        };
        let prefs = effects::prefs::load(&path);
        self.prefs_path = Some(path);
        self.dispatch_local(Msg::Shell(ShellMsg::ApplyPrefs(prefs)));
    }

    /// Dispatches a message through the pure reducer and executes every
    /// returned [`crate::effects::Effect`] with access to Rune's [`Tx`].
    pub fn dispatch(&mut self, msg: Msg, tx: &mut Tx) {
        self.sync_draft_text();
        let active = self.model.terminal.active;
        let created = matches!(
            msg,
            Msg::Terminal(update::terminal::TerminalMsg::Created(..))
        );
        let in_terminal = tx.focused().is_some_and(|key| key.starts_with("terminal-"));
        let effects = update::update(&mut self.model, msg);
        crate::git::diff_panel::sync(self);
        crate::terminal::panel::sync(self);
        crate::requests::sync(self);
        if self.model.panels.terminal_open
            && active != self.model.terminal.active
            && (created || in_terminal)
        {
            crate::terminal::panel::focus_active(self, tx);
        }
        self.apply_effects(effects, Some(tx));
    }

    /// Dispatches a synchronous UI message from handlers that do not receive
    /// [`Tx`].
    pub fn dispatch_local(&mut self, msg: Msg) {
        self.sync_draft_text();
        let effects = update::update(&mut self.model, msg);
        crate::git::diff_panel::sync(self);
        crate::terminal::panel::sync(self);
        crate::requests::sync(self);
        self.apply_effects(effects, None);
    }

    /// Window focus hook called by Rune's window lifecycle and terminal scene.
    pub fn set_focus(&mut self, focused: bool, tx: &mut Tx) {
        self.dispatch(Msg::Shell(ShellMsg::WindowFocus(focused)), tx);
    }

    /// Pure cursor snapshot of the main composer [`TextEdit`].
    #[must_use]
    pub const fn draft_cursor(&self) -> DraftCursor {
        DraftCursor {
            focused: self.widgets.draft.focused,
            anchor: self.widgets.draft.anchor,
            focus: self.widgets.draft.focus,
        }
    }

    /// Synchronizes all Rune widget controllers from [`Self::model`] after a
    /// headless scene populates fixture state directly.
    pub fn sync_widgets_from_model(&mut self) {
        crate::git::diff_panel::sync(self);
        crate::terminal::panel::sync(self);
        crate::requests::sync(self);
        self.widgets.git_query = TextEdit::new(&self.model.git.query);
        self.widgets.draft = TextEdit::new(&self.model.composer.draft.text);
        self.widgets.draft.focused = self.model.composer.draft_focused;

        self.widgets.project_filter_query = TextEdit::new(&self.model.sidebar.filter_query);
        self.widgets.project_filter_query.focused = self.model.sidebar.filter_open;
        if let Some(rename) = &self.model.sidebar.inline_rename {
            self.widgets.inline_rename = TextEdit::new(&rename.value);
            self.widgets.inline_rename.focused = true;
            self.widgets
                .inline_rename
                .set_selection(0, rename.value.len());
        } else {
            self.widgets.inline_rename = TextEdit::new("");
        }
        self.widgets.sidebar_dialog = TextEdit::new(&self.model.sidebar.edit_value);

        self.widgets.palette_query = TextEdit::new(&self.model.palette.query);
        self.widgets.palette_query.focused = self.model.palette.open;

        self.widgets.model_query = TextEdit::new(&self.model.composer.model_menu.query);
        self.widgets.model_query.focused =
            self.model.composer.open_picker.as_deref() == Some("model");

        self.widgets
            .sidebar_pane
            .set_width(self.model.panels.sidebar.width);
        self.widgets
            .sidebar_pane
            .set_open(self.model.panels.sidebar.open);
        self.widgets
            .diff_pane
            .set_width(self.model.panels.diff.width);
        self.widgets.diff_pane.set_open(self.model.panels.diff.open);
    }

    /// Adds a persistent toast for preview scenes (no expiry clock).
    pub fn preview_toast(&mut self, title: &str, detail: &str) {
        let key = format!("{title}\n{detail}");
        let _ = self.widgets.toasts.step(ToastEvent::Shown {
            key,
            options: ToastContent {
                title: title.into(),
                detail: Some(detail.into()),
                tone: Tone::Danger,
                undo: None,
            },
            secs: None,
            now: 0.0,
        });
    }

    /// Adds a persistent triage undo toast for preview scenes.
    pub fn preview_undo_toast(&mut self, title: &str, detail: &str, undo: UndoTriage) {
        let key = format!("undo-{}-{title}", undo.thread);
        let _ = self.widgets.toasts.step(ToastEvent::Shown {
            key,
            options: ToastContent {
                title: title.into(),
                detail: Some(detail.into()),
                tone: Tone::Neutral,
                undo: Some(undo),
            },
            secs: None,
            now: 0.0,
        });
    }

    /// Returns the decoded image handle for `project` in the active
    /// environment, if loaded.
    #[must_use]
    pub fn project_icon_image(&self, project: &str) -> Option<&Image> {
        let key = format!("{}\0{project}", self.model.active_env_key());
        self.widgets.project_icons.get(&key)
    }

    /// Decodes fetched project icon bytes on the image cache worker pool.
    pub(crate) fn decode_project_icon(
        &mut self,
        environment: &str,
        project: &str,
        media_type: &str,
        bytes: Vec<u8>,
    ) {
        let cache = self
            .widgets
            .images
            .get_or_insert_with(|| ImageCache::new(Arc::new(|| {})));
        let image = cache.load_mime(bytes, media_type, 24.0, 2.0);
        let key = format!("{environment}\0{project}");
        self.widgets.project_icons.insert(key, image);
    }

    /// Closes a toast early.
    pub fn dismiss_toast(&mut self, id: u64, tx: &mut Tx) {
        self.step_toasts(
            ToastEvent::Dismissed {
                id: Some(id),
                now: tx.clock(),
            },
            tx,
        );
    }

    /// Pauses or resumes toast auto-dismiss while the pointer is over the
    /// stack.
    pub fn toast_hover(&mut self, inside: bool, tx: &mut Tx) {
        let event = if inside {
            ToastEvent::PointerOver { now: tx.clock() }
        } else {
            ToastEvent::PointerLeft { now: tx.clock() }
        };
        self.step_toasts(event, tx);
    }

    /// Syncs window visibility and expires due toasts; returns true if any
    /// closed.
    pub fn expire_toasts(&mut self, clock: f64, tx: &mut Tx) -> bool {
        if self.widgets.toasts.entries().is_empty() {
            return false;
        }
        let page = if tx.window_visible() {
            ToastPage::Shown
        } else {
            ToastPage::Hidden
        };
        if self.widgets.toasts.page() != page {
            self.step_toasts(ToastEvent::Page { page, now: clock }, tx);
        }
        if self
            .widgets
            .toasts
            .next_due()
            .is_some_and(|due| clock + 1e-3 >= due)
        {
            return self.step_toasts(ToastEvent::Tick { now: clock }, tx);
        }
        false
    }

    pub(crate) fn prefs_path(&self) -> Option<&Path> {
        self.prefs_path.as_deref()
    }

    pub(crate) fn send_command(&self, command: ServerEffect) -> Option<ServerEffect> {
        self.commands.send(command).err().map(|error| error.0)
    }

    pub(crate) fn show_toast(
        &mut self,
        tx: &mut Tx,
        title: impl Into<String>,
        detail: Option<String>,
    ) {
        let title = title.into();
        let key = format!("{title}\n{}", detail.as_deref().unwrap_or_default());
        let tone = if detail.is_some() {
            Tone::Danger
        } else {
            Tone::Neutral
        };
        self.step_toasts(
            ToastEvent::Shown {
                key,
                options: ToastContent {
                    title,
                    detail,
                    tone,
                    undo: None,
                },
                secs: Some(TOAST_LIFE.as_secs_f64()),
                now: tx.clock(),
            },
            tx,
        );
    }

    pub(crate) fn show_undo_toast(
        &mut self,
        tx: &mut Tx,
        title: String,
        detail: Option<String>,
        undo: UndoTriage,
        secs: f64,
    ) {
        let key = format!("undo-{}", undo.thread);
        self.step_toasts(
            ToastEvent::Shown {
                key,
                options: ToastContent {
                    title,
                    detail,
                    tone: Tone::Neutral,
                    undo: Some(undo),
                },
                secs: Some(secs),
                now: tx.clock(),
            },
            tx,
        );
    }

    fn sync_draft_text(&mut self) {
        if self.widgets.draft.text != self.model.composer.draft.text {
            self.model
                .composer
                .draft
                .text
                .clone_from(&self.widgets.draft.text);
        }
        self.model.composer.draft_focused = self.widgets.draft.focused;
    }

    fn step_toasts(&mut self, event: ToastEvent<ToastContent>, tx: &mut Tx) -> bool {
        let mut closed = false;
        for effect in self.widgets.toasts.step(event) {
            match effect {
                ToastEffect::Closed(_) => closed = true,
                ToastEffect::WakeAt(due) => {
                    let wait = (due - tx.clock()).max(0.0);
                    tx.wake_after(Duration::from_secs_f64(wait));
                }
                ToastEffect::Pulse(_) => {}
            }
        }
        closed
    }
}

/// Converts the pure [`PaneSwitch`] direction to [`crate::motion::Switch`].
#[must_use]
pub const fn motion_switch(switch: PaneSwitch) -> Switch {
    match switch {
        PaneSwitch::Cut => Switch::Cut,
        PaneSwitch::Up => Switch::Up,
        PaneSwitch::Down => Switch::Down,
        PaneSwitch::Into => Switch::Into,
        PaneSwitch::Out => Switch::Out,
        PaneSwitch::Fade => Switch::Fade,
    }
}
