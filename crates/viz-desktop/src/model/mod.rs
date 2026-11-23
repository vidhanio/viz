//! Pure domain and client state for `viz-desktop`.
//!
//! Nothing under `model` imports Rune, Tokio, channels, or performs I/O. Every
//! state container is plain data and every projection or derivation is a pure
//! function.

pub mod cache;
pub mod completion;
pub mod composer_state;
pub mod git;
pub mod palette;
pub mod picker;
pub mod prefs;
pub mod sidebar;
pub mod state;
pub mod subagents;
pub mod timeline;
pub mod tool_parse;
pub mod transcript;

use std::{collections::BTreeMap, sync::OnceLock};

use viz_client::protocol::{
    Harness, Project, Task, ThreadId, ThreadState, ThreadStatus, ThreadSummary,
};

pub use self::{
    cache::{Checkouts, EnvironmentCache, OpenThread},
    completion::{
        completion_token_at, format_completion_insertion, matching_commands, matching_files,
    },
    composer_state::{
        AttachmentTarget, CompletionKind, CompletionToken, ComposerState, DraftCursor, DraftData,
        ModelMenuState, PaletteChoice, PaletteCommand, PaletteSearch, PaletteState,
    },
    palette::{PaletteRow, palette_rows},
    picker::{
        ModelEntry, ModelLevels, ModelPicker, OptionOwner, OptionRole, OptionTarget, model_groups,
        option_role, provider_name, thinking_ids, unseen_model_levels,
    },
    prefs::{Prefs, SidebarMode},
    sidebar::{SidebarOutline, sidebar_first_match, sidebar_position, top_level},
    state::{
        ConnectionState, ContextMenu, DIFF_DEFAULT_WIDTH, DIFF_MAX_WIDTH, DIFF_MIN_WIDTH,
        DisclosureState, NewThread, OptimisticLedger, OptimisticUndo, PaneSwitch, PanelsState,
        SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SelectionState, SidebarEdit,
        SidebarState, Status, TERMINAL_DEFAULT_HEIGHT, WINDOW_SIZE,
    },
    subagents::{
        AgentStanding, SubagentCall, child_link, delegated_task, local_child, spawned_subagent,
        subagent_call, subagent_title,
    },
    timeline::{
        ActivityMark, ActivityTally, ToolAction, first_non_empty_line, humanize_tool, is_activity,
        relative_path, split_path,
    },
    tool_parse::is_command,
    transcript::{Item, PlanStep, TaskResult, Tool, ToolStatus, Transcript, TurnEnd, User},
};

/// Default key used in per-environment maps before the server reports an
/// environment id in `hello`.
pub const DEFAULT_ENV_KEY: &str = "local";

/// Complete pure state of the desktop workspace.
#[derive(Debug, Clone)]
pub struct Model {
    pub git: git::GitState,
    /// Connection status, endpoint, and environment roster.
    pub connection: ConnectionState,
    /// Normalised server cache per environment id.
    pub caches: BTreeMap<String, EnvironmentCache>,
    /// Active thread or draft and back/forward history.
    pub selection: SelectionState,
    /// Shell slot visibility, pane widths, and window width.
    pub panels: PanelsState,
    /// Sidebar filter, mode, collapses, menus, and dialogs.
    pub sidebar: SidebarState,
    /// Composer draft, saved drafts, completions, pickers, and favorites.
    pub composer: ComposerState,
    /// Command palette modal state.
    pub palette: PaletteState,
    /// Timeline disclosure toggles, tool clock, and window focus.
    pub disclosure: DisclosureState,
    /// In-flight optimistic updates carrying undo snapshots for rollback.
    pub optimistic: OptimisticLedger,
}

impl Default for Model {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Model {
    /// Constructs a clean initial model with `tool_clock` initialized to `now`.
    #[must_use]
    pub fn new(now_ms: i64) -> Self {
        let mut model = Self {
            git: git::GitState::default(),
            connection: ConnectionState::default(),
            caches: BTreeMap::new(),
            selection: SelectionState::default(),
            panels: PanelsState::default(),
            sidebar: SidebarState::default(),
            composer: ComposerState::default(),
            palette: PaletteState::default(),
            disclosure: DisclosureState {
                tool_clock: now_ms,
                ..DisclosureState::default()
            },
            optimistic: OptimisticLedger::default(),
        };
        model.composer.draft_focused = true;
        model
    }

    /// Active environment key used to index [`Self::caches`] and saved drafts.
    #[must_use]
    pub fn active_env_key(&self) -> &str {
        self.connection
            .environment
            .as_deref()
            .unwrap_or(DEFAULT_ENV_KEY)
    }

    /// Active environment's normalised server cache.
    #[must_use]
    pub fn cache(&self) -> &EnvironmentCache {
        static EMPTY: OnceLock<EnvironmentCache> = OnceLock::new();
        self.caches
            .get(self.active_env_key())
            .unwrap_or_else(|| EMPTY.get_or_init(EnvironmentCache::default))
    }

    /// Mutable reference to the active environment's cache, creating it if
    /// absent.
    pub fn cache_mut(&mut self) -> &mut EnvironmentCache {
        let key = self.active_env_key().to_owned();
        self.caches.entry(key).or_default()
    }

    /// Whether the client is connected to a server.
    #[must_use]
    pub const fn connected(&self) -> bool {
        matches!(self.connection.status, Status::Connected { .. })
    }

    /// Delegated tasks in the active environment.
    #[must_use]
    pub fn tasks(&self) -> &[Task] {
        &self.cache().tasks
    }

    /// Harnesses advertised by the active environment.
    #[must_use]
    pub fn harnesses(&self) -> &[Harness] {
        &self.cache().harnesses
    }

    /// Branches and worktrees cached for `project_path`, if loaded.
    #[must_use]
    pub fn checkouts(&self, project_path: &str) -> Option<&Checkouts> {
        self.cache().checkouts.get(project_path)
    }

    /// Cached open-thread record for the currently selected thread.
    #[must_use]
    pub fn open_thread(&self) -> Option<&OpenThread> {
        let id = self.selection.selected?;
        self.cache().open_thread(id)
    }

    /// Transcript of the selected thread, or an empty transcript when none is
    /// selected or loaded.
    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        static EMPTY: OnceLock<Transcript> = OnceLock::new();
        self.open_thread().map_or_else(
            || EMPTY.get_or_init(Transcript::default),
            |open| &open.transcript,
        )
    }

    /// Watched thread state for the selected thread, if received.
    #[must_use]
    pub fn thread_state(&self) -> Option<&ThreadState> {
        self.open_thread().and_then(|open| open.state.as_ref())
    }

    /// Whether the selected thread's initial transcript has not yet arrived.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.open_thread().is_some_and(|open| open.loading)
    }

    /// Whether an older page of entries is currently being fetched.
    #[must_use]
    pub fn is_history_loading(&self) -> bool {
        self.open_thread().is_some_and(|open| open.history_loading)
    }

    /// Summary of `thread` in the active environment, once listed.
    #[must_use]
    pub fn summary(&self, thread: ThreadId) -> Option<&ThreadSummary> {
        self.cache().summary(thread)
    }

    /// Display title for `thread` in the sidebar and header.
    #[must_use]
    pub fn title(&self, thread: ThreadId) -> &str {
        self.summary(thread).map_or("New thread", thread_title)
    }

    /// Project path of the active draft or selected thread.
    #[must_use]
    pub fn project_path(&self) -> Option<&str> {
        if let Some(thread) = &self.selection.new_thread {
            return thread.project.as_deref();
        }
        let id = self.selection.selected?;
        self.summary(id).map_or_else(
            || self.thread_state().and_then(|s| s.project.as_deref()),
            |summary| summary.project.as_deref(),
        )
    }

    /// Project record for the active draft or selected thread.
    #[must_use]
    pub fn project(&self) -> Option<&Project> {
        let path = self.project_path()?;
        self.cache().project(path)
    }

    /// Whether the currently selected thread has an active running turn.
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(
            self.thread_state().map(|s| s.status),
            Some(ThreadStatus::Working)
        )
    }

    /// Current composer target accepting attachments, if ready.
    #[must_use]
    pub fn attachment_target(&self) -> Option<AttachmentTarget> {
        if !self.connected() {
            return None;
        }
        if let Some(thread) = &self.selection.new_thread {
            return (!thread.sending).then_some(AttachmentTarget::Draft(thread.id));
        }
        self.thread_state()?;
        self.selection.selected.map(AttachmentTarget::Thread)
    }

    /// Whether the composer can start attaching files right now.
    #[must_use]
    pub fn can_attach(&self) -> bool {
        self.attachment_target()
            .is_some_and(|target| !self.composer.attachment_reads.contains(&target))
    }

    /// Whether the composer can submit the current draft.
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.can_attach()
            && self.selection.new_thread.as_ref().is_none_or(|thread| {
                !thread.loading_choices && thread.active_harness(self.harnesses()).is_some()
            })
            && (!self.composer.draft.text.trim().is_empty()
                || !self.composer.draft.attachments.is_empty())
    }

    /// Whether any non-archived thread in the active environment is unread.
    #[must_use]
    pub fn has_unread(&self) -> bool {
        self.cache()
            .threads()
            .any(|thread| thread.lifecycle.unread && !thread.lifecycle.archived)
    }

    /// Window title reflecting connection and active thread run state.
    #[must_use]
    pub fn window_title(&self) -> &'static str {
        match (
            &self.connection.status,
            self.thread_state().map(|s| s.status),
        ) {
            (Status::Error(_), _) => "viz — Offline",
            (Status::Connecting, _) => "viz — Connecting",
            (_, Some(ThreadStatus::Working)) => "viz — Working",
            (_, Some(ThreadStatus::Failed)) => "viz — Failed",
            _ => "viz",
        }
    }

    /// Whether `thread` was delegated, directly or indirectly, by `ancestor`.
    #[must_use]
    pub fn descends(&self, thread: ThreadId, ancestor: ThreadId) -> bool {
        let mut seen = 0;
        let limit = self.cache().thread_count();
        let mut at = self.summary(thread).and_then(|summary| summary.parent);
        while let Some(parent) = at {
            if parent == ancestor {
                return true;
            }
            seen += 1;
            if seen > limit {
                return false;
            }
            at = self.summary(parent).and_then(|summary| summary.parent);
        }
        false
    }

    /// Spatial travel direction for switching from `from` to `to`.
    #[must_use]
    pub fn switch_direction(&self, from: ThreadId, to: ThreadId) -> PaneSwitch {
        if self.descends(to, from) {
            return PaneSwitch::Into;
        }
        if self.descends(from, to) {
            return PaneSwitch::Out;
        }
        match (sidebar_position(self, from), sidebar_position(self, to)) {
            (Some(a), Some(b)) if b > a => PaneSwitch::Down,
            (Some(a), Some(b)) if b < a => PaneSwitch::Up,
            _ => PaneSwitch::Fade,
        }
    }

    /// Applies loaded client preferences onto the model.
    pub fn apply_prefs(&mut self, prefs: &Prefs) {
        self.sidebar.mode = prefs.sidebar;
        self.sidebar
            .collapsed_projects
            .clone_from(&prefs.collapsed_projects);
        self.sidebar
            .collapsed_shelves
            .clone_from(&prefs.collapsed_shelves);
        if let Some(width) = prefs.sidebar_width {
            self.panels.sidebar.width = width
                .round()
                .clamp(self.panels.sidebar.min, self.panels.sidebar.max);
        }
        if let Some(open) = prefs.sidebar_open {
            self.panels.sidebar.open = open;
        }
        if let Some(width) = prefs.diff_width {
            self.panels.diff.width = width
                .round()
                .clamp(self.panels.diff.min, self.panels.diff.max);
        }
        if let Some(height) = prefs.terminal_height {
            self.panels.terminal_height = height.round().clamp(120.0, 600.0);
        }
        self.composer
            .favorite_models
            .clone_from(&prefs.favorite_models);
        for (env, threads) in &prefs.drafts {
            let env_map = self.composer.drafts_by_env.entry(env.clone()).or_default();
            for (thread_key, text) in threads {
                if let Ok(raw_id) = thread_key.parse::<u64>()
                    && !text.is_empty()
                {
                    env_map
                        .entry(ThreadId(raw_id))
                        .or_insert_with(|| DraftData {
                            text: text.clone(),
                            attachments: Vec::new(),
                        });
                }
            }
        }
    }

    /// Projects persistable client preferences from the current model state.
    #[must_use]
    pub fn to_prefs(&self) -> Prefs {
        let mut drafts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (env, threads) in &self.composer.drafts_by_env {
            let mut env_drafts = BTreeMap::new();
            for (id, draft) in threads {
                if !draft.text.is_empty() {
                    env_drafts.insert(id.0.to_string(), draft.text.clone());
                }
            }
            if !env_drafts.is_empty() {
                drafts.insert(env.clone(), env_drafts);
            }
        }
        if let Some(id) = self.selection.selected
            && !self.composer.draft.text.is_empty()
        {
            drafts
                .entry(self.active_env_key().to_owned())
                .or_default()
                .insert(id.0.to_string(), self.composer.draft.text.clone());
        }
        Prefs {
            version: prefs::PREFS_VERSION,
            sidebar: self.sidebar.mode,
            sidebar_width: Some(self.panels.sidebar.width),
            sidebar_open: Some(self.panels.sidebar.open),
            diff_width: (self.panels.diff.width - DIFF_DEFAULT_WIDTH)
                .abs()
                .ge(&1.0)
                .then_some(self.panels.diff.width),
            terminal_height: (self.panels.terminal_height - TERMINAL_DEFAULT_HEIGHT)
                .abs()
                .ge(&1.0)
                .then_some(self.panels.terminal_height),
            collapsed_projects: self.sidebar.collapsed_projects.clone(),
            collapsed_shelves: self.sidebar.collapsed_shelves.clone(),
            favorite_models: self.composer.favorite_models.clone(),
            drafts,
            window: None,
        }
    }
}

/// Display title of a thread summary, falling back to `"New thread"`.
#[must_use]
pub fn thread_title(thread: &ThreadSummary) -> &str {
    if !thread.title.is_empty() {
        return &thread.title;
    }
    "New thread"
}
