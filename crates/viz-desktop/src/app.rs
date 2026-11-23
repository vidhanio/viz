//! Imperative shell wrapper around [`Model`]: owns Rune widget state, exposes
//! effect sinks, and funnels every UI event through [`Desktop::dispatch`].

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use rune::{ImageCache, TextEdit, Tx, image::Image};
use rune_kit::{ImageZoom, ToastStack, shell::PaneState};
use tokio::sync::mpsc;

use crate::{
    effects::{self, ServerEffect},
    model::{
        DIFF_DEFAULT_WIDTH, DIFF_MAX_WIDTH, DIFF_MIN_WIDTH, DraftCursor, DraftPill, Model,
        PaneSwitch, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH,
    },
    motion::Switch,
    motion_scene::MotionDemo,
    theme,
    update::{self, Msg, ShellMsg},
};

mod toasts;

pub use toasts::ToastContent;

/// Stateful Rune and `rune-kit` widget controllers kept outside the pure
/// [`Model`].
pub struct ShellWidgets {
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
    pub subagent_hover: Option<String>,
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
    /// Home hero project picker search input.
    pub home_project_query: TextEdit,
    /// Home context strip branch search input.
    pub draft_branch_query: TextEdit,
    /// Attachment image lightbox zoom and pan controller.
    pub image_zoom: ImageZoom,
    /// Resizable, collapsible sidebar pane state.
    pub sidebar_pane: PaneState,
    /// Resizable, collapsible docked diff inspector pane state.
    pub diff_pane: PaneState,
    /// Coalesced toast stack with hover and hidden-window timer pause.
    pub toasts: ToastStack<ToastContent>,
    /// Toasts whose close button was pressed, dismissed on the next poll
    /// (`rune_kit::Toast::on_close` runs without a [`Tx`]).
    pub closed_toasts: Vec<u64>,
    /// Off-thread image decoder cache initialized in `AppSpec::setup`.
    pub images: Option<ImageCache>,
    /// Decoded project icon images keyed by `"{env}\0{project_path}"`.
    pub project_icons: HashMap<String, Image>,
    /// Decoded images for the active draft, in attachment order.
    pub attachment_images: crate::attachments::DraftImages,
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
            subagent_hover: None,
            draft,
            project_filter_query: TextEdit::new(""),
            inline_rename: TextEdit::new(""),
            sidebar_dialog: TextEdit::new(""),
            palette_query: TextEdit::new(""),
            model_query: TextEdit::new(""),
            home_project_query: TextEdit::new(""),
            draft_branch_query: TextEdit::new(""),
            image_zoom: ImageZoom::new(),
            sidebar_pane: PaneState::new(SIDEBAR_DEFAULT_WIDTH)
                .range(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH),
            diff_pane: PaneState::new(DIFF_DEFAULT_WIDTH)
                .range(DIFF_MIN_WIDTH, DIFF_MAX_WIDTH)
                .collapsed(),
            toasts: ToastStack::default(),
            closed_toasts: Vec::new(),
            images: None,
            project_icons: HashMap::new(),
            attachment_images: crate::attachments::DraftImages::default(),
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
        self.model.palette.pages.clock = tx.clock();
        let effects = update::update(&mut self.model, msg);
        crate::git::diff_panel::sync(self);
        crate::requests::sync(self);
        self.decode_draft_attachments();
        self.apply_effects(effects, Some(tx));
    }

    /// Dispatches a synchronous UI message from handlers that do not receive
    /// [`Tx`].
    pub fn dispatch_local(&mut self, msg: Msg) {
        self.sync_draft_text();
        let effects = update::update(&mut self.model, msg);
        crate::git::diff_panel::sync(self);
        crate::requests::sync(self);
        self.apply_effects(effects, None);
    }

    /// Window focus hook called by Rune's window lifecycle.
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
        crate::requests::sync(self);
        self.widgets.git_query = TextEdit::new(&self.model.git.query);
        let draft_text = self.model.composer.draft.text.clone();
        let draft_pills = self.model.composer.draft.pills.clone();
        self.widgets.draft = crate::attachments::build_draft_edit(&draft_text, &draft_pills);
        self.widgets.draft.focused = self.model.composer.draft_focused;

        self.widgets.home_project_query = TextEdit::new(&self.model.composer.home_project.query);
        self.widgets.home_project_query.focused =
            self.model.composer.open_picker.as_deref() == Some(update::HOME_PROJECT_PICKER);
        self.widgets.draft_branch_query = TextEdit::new(&self.model.composer.draft_branch.query);
        self.widgets.draft_branch_query.focused =
            self.model.composer.open_picker.as_deref() == Some(update::DRAFT_BRANCH_PICKER);
        self.decode_draft_attachments();

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

        let query = self
            .model
            .palette
            .pages
            .stack
            .last()
            .map_or(&self.model.palette.query, |page| &page.query);
        self.widgets.palette_query = TextEdit::new(query);
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

    pub(crate) fn prefs_path(&self) -> Option<&Path> {
        self.prefs_path.as_deref()
    }

    pub(crate) fn send_command(&self, command: ServerEffect) -> Option<ServerEffect> {
        self.commands.send(command).err().map(|error| error.0)
    }

    /// Returns the decoded thumbnail image handle for an image attachment, if
    /// available.
    #[must_use]
    pub fn attachment_image(&self, index: usize) -> Option<&Image> {
        self.widgets.attachment_images.get(index)
    }

    pub(crate) fn decode_draft_attachments(&mut self) {
        self.widgets.attachment_images.sync(
            &self.model.composer.draft.attachments,
            &mut self.widgets.images,
        );
    }

    pub(crate) fn replace_draft_with_pills(&mut self, text: &str, pills: &[DraftPill]) {
        self.widgets.draft = crate::attachments::build_draft_edit(text, pills);
        self.sync_draft_text();
    }

    pub(crate) fn sync_draft_text(&mut self) {
        crate::attachments::sync_draft_text(self);
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
