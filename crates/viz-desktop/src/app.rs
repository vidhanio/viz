//! Imperative shell wrapper around [`Model`]: owns Rune widget state, runs
//! [`Effect`]s, and funnels every UI event through [`Desktop::dispatch`].

use std::{path::PathBuf, time::Duration};

use rune::{FileDialogOptions, TextEdit, Tx};
use rune_kit::{ToastEffect, ToastEvent, ToastPage, ToastStack, shell::PaneState};
use tokio::sync::mpsc;

use crate::{
    effects::{self, EditSync, EditTarget, Effect, ServerEffect},
    model::{
        DIFF_DEFAULT_WIDTH, DIFF_MAX_WIDTH, DIFF_MIN_WIDTH, DraftCursor, Model, PaneSwitch,
        SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
    },
    motion::Switch,
    motion_scene::MotionDemo,
    theme,
    update::{self, Msg, ServerMsg, ShellMsg, SidebarMsg},
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
}

/// Stateful Rune and `rune-kit` widget controllers kept outside the pure
/// [`Model`].
#[derive(Debug)]
pub struct ShellWidgets {
    /// Main composer text editor.
    pub draft: TextEdit,
    /// Sidebar thread filter input.
    pub sidebar_filter: TextEdit,
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
}

impl Default for ShellWidgets {
    fn default() -> Self {
        let mut draft = TextEdit::new("");
        draft.focused = true;
        Self {
            draft,
            sidebar_filter: TextEdit::new(""),
            sidebar_dialog: TextEdit::new(""),
            palette_query: TextEdit::new(""),
            model_query: TextEdit::new(""),
            sidebar_pane: PaneState::new(SIDEBAR_DEFAULT_WIDTH)
                .range(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            diff_pane: PaneState::new(DIFF_DEFAULT_WIDTH)
                .range(DIFF_MIN_WIDTH, DIFF_MAX_WIDTH)
                .collapsed(),
            toasts: ToastStack::default(),
        }
    }
}

/// Root Rune application state: pure [`Model`] plus shell widgets and effect
/// sinks.
#[derive(Debug)]
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
    /// returned [`Effect`] with access to Rune's [`Tx`].
    pub fn dispatch(&mut self, msg: Msg, tx: &mut Tx) {
        self.sync_draft_text();
        let effects = update::update(&mut self.model, msg);
        self.apply_effects(effects, Some(tx));
    }

    /// Dispatches a synchronous UI message from handlers that do not receive
    /// [`Tx`].
    pub fn dispatch_local(&mut self, msg: Msg) {
        self.sync_draft_text();
        let effects = update::update(&mut self.model, msg);
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
        self.widgets.draft = TextEdit::new(&self.model.composer.draft.text);
        self.widgets.draft.focused = self.model.composer.draft_focused;

        self.widgets.sidebar_filter = TextEdit::new(&self.model.sidebar.filter);
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
            },
            secs: None,
            now: 0.0,
        });
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

    #[expect(
        clippy::too_many_lines,
        reason = "single exhaustive interpreter over every Effect variant"
    )]
    fn apply_effects(&mut self, effects: Vec<Effect>, mut tx: Option<&mut Tx>) {
        for effect in effects {
            match effect {
                Effect::Server(command) => self.send_server(command, tx.as_deref_mut()),
                Effect::Attach {
                    scope,
                    target,
                    source,
                } => self.send_server(
                    ServerEffect::Attach {
                        scope,
                        target,
                        source,
                    },
                    tx.as_deref_mut(),
                ),
                Effect::Notify { title, body } => {
                    self.send_server(ServerEffect::Notify { title, body }, tx.as_deref_mut());
                }
                Effect::SavePrefs(prefs) => {
                    if let Some(path) = &self.prefs_path {
                        effects::prefs::save(path, &prefs);
                    }
                }
                Effect::PickProjectFolder => {
                    if let Some(tx) = tx.as_deref_mut() {
                        let options = FileDialogOptions {
                            title: "Add project".into(),
                            ..FileDialogOptions::default()
                        };
                        tx.open_folder(options, |s: &mut Self, result, tx| match result {
                            Ok(Some(path)) => s.dispatch(
                                Msg::Sidebar(SidebarMsg::ProjectFolderPicked(
                                    path.to_string_lossy().into_owned(),
                                )),
                                tx,
                            ),
                            Ok(None) => {}
                            Err(error) => s.dispatch(
                                Msg::Sidebar(SidebarMsg::ProjectFolderFailed(error.to_string())),
                                tx,
                            ),
                        });
                    }
                }
                Effect::CopyText(text) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.copy(text);
                    }
                }
                Effect::OpenUrl(url) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        _ = tx.open_url(&url);
                    }
                }
                Effect::SetWindowTitle(title) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.set_title(title);
                    }
                }
                Effect::Toast { title, detail } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        self.show_toast(tx, title, detail);
                    }
                }
                Effect::Focus {
                    key,
                    select_all,
                    when_shown,
                } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        if when_shown {
                            tx.focus_when_shown(key, select_all);
                        } else {
                            tx.focus(key, select_all);
                        }
                    }
                }
                Effect::ScrollToBottom(key) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_to_bottom(key);
                    }
                }
                Effect::ScrollToInstant { key, x, y } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_to_instant(key, x, y);
                    }
                }
                Effect::ScrollIntoView { list, item } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_into_view_in(list, item);
                    }
                }
                Effect::ScrollCenterInstant { list, item } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_center_in_instant(list, item);
                    }
                }
                Effect::KeepOnScreen(key) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.keep_on_screen(key);
                    }
                }
                Effect::WakeAfter(duration) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.wake_after(duration);
                    }
                }
                Effect::SyncEdit(sync) => self.apply_edit_sync(sync),
                Effect::SyncSidebarPane { open, width } => {
                    if let Some(width) = width {
                        self.widgets.sidebar_pane.set_width(width);
                    }
                    if let Some(open) = open {
                        self.widgets.sidebar_pane.set_open(open);
                    }
                }
                Effect::SyncDiffPane { open, width } => {
                    if let Some(width) = width {
                        self.widgets.diff_pane.set_width(width);
                    }
                    if let Some(open) = open {
                        self.widgets.diff_pane.set_open(open);
                    }
                }
            }
        }
    }

    fn send_server(&mut self, command: ServerEffect, mut tx: Option<&mut Tx>) {
        if let Err(error) = self.commands.send(command) {
            let recovery = match error.0 {
                ServerEffect::Create { draft, params } => Some(ServerMsg::LaunchFailed {
                    draft,
                    params: Box::new(params),
                    title: "Offline".into(),
                    detail: None,
                }),
                ServerEffect::SendInput(input) => Some(ServerMsg::SendFailed {
                    input: Box::new(input),
                    title: "Offline".into(),
                    detail: None,
                }),
                ServerEffect::Update { op_id, .. }
                | ServerEffect::Project {
                    op_id: Some(op_id), ..
                } => Some(ServerMsg::MutationFailed {
                    op_id,
                    title: "Offline".into(),
                    detail: "Server disconnected".into(),
                }),
                ServerEffect::LoadDraft(draft) => Some(ServerMsg::DraftHarnessesLoaded {
                    draft,
                    result: None,
                }),
                ServerEffect::SearchThreads { request, .. } => {
                    Some(ServerMsg::ThreadSearchResult {
                        request,
                        result: Err("The server is offline.".into()),
                    })
                }
                _ => {
                    if let Some(tx) = tx.as_deref_mut() {
                        self.show_toast(tx, "Offline", None);
                    }
                    None
                }
            };
            if let Some(msg) = recovery {
                let follow_up = update::update(&mut self.model, Msg::Server(msg));
                self.apply_effects(follow_up, tx);
            }
        }
    }

    fn apply_edit_sync(&mut self, sync: EditSync) {
        match sync {
            EditSync::Replace {
                target,
                text,
                select_all,
            } => {
                let edit = self.edit_mut(target);
                edit.replace_text(&text);
                if select_all {
                    edit.set_selection(0, text.len());
                }
            }
            EditSync::Clear(target) => {
                self.edit_mut(target).clear();
            }
            EditSync::PrependDraft(text) => {
                let draft = &mut self.widgets.draft;
                if !text.is_empty() {
                    if !draft.text.is_empty() {
                        draft.set_caret(0);
                        draft.insert("\n\n");
                        draft.set_caret(0);
                    }
                    draft.insert(&text);
                }
            }
            EditSync::ReplaceDraftRange {
                start,
                end,
                insertion,
            } => {
                let draft = &mut self.widgets.draft;
                draft.set_selection(start, end);
                draft.insert(&insertion);
                if !insertion.ends_with(' ')
                    && let Some(separator) = draft.text[draft.focus..].chars().next()
                {
                    draft.set_caret(draft.focus + separator.len_utf8());
                }
                self.model.composer.draft.text.clone_from(&draft.text);
            }
        }
    }

    const fn edit_mut(&mut self, target: EditTarget) -> &mut TextEdit {
        match target {
            EditTarget::Draft => &mut self.widgets.draft,
            EditTarget::SidebarFilter => &mut self.widgets.sidebar_filter,
            EditTarget::SidebarDialog => &mut self.widgets.sidebar_dialog,
            EditTarget::PaletteQuery => &mut self.widgets.palette_query,
            EditTarget::ModelQuery => &mut self.widgets.model_query,
        }
    }

    fn show_toast(&mut self, tx: &mut Tx, title: impl Into<String>, detail: Option<String>) {
        let title = title.into();
        let key = format!("{title}\n{}", detail.as_deref().unwrap_or_default());
        self.step_toasts(
            ToastEvent::Shown {
                key,
                options: ToastContent { title, detail },
                secs: Some(TOAST_LIFE.as_secs_f64()),
                now: tx.clock(),
            },
            tx,
        );
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
