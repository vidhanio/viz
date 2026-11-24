//! Pure domain and client state for `viz-desktop`.
//!
//! Nothing under `model` imports Rune, Tokio, channels, or performs I/O. Every
//! state container is plain data and every projection or derivation is a pure
//! function.

pub mod cache;
pub mod completion;
mod completion_match;
pub mod composer_state;
pub mod git;
pub mod home;
pub mod model_list;
pub mod model_menu;
pub mod model_options;
pub mod palette;
pub mod picker;
pub mod prefs;
pub mod projects;
pub mod requests;
pub mod sidebar;
pub mod sidebar_outline;
pub mod state;
pub mod subagents;
pub mod timeline;
pub mod tool_parse;
pub mod transcript;
mod transcript_fold;
pub mod turn_format;
pub mod turns;

use std::{collections::BTreeMap, sync::OnceLock};

use viz_client::protocol::{
    Harness, Project, Task, ThreadId, ThreadState, ThreadStatus, ThreadSummary,
};

pub use self::{
    cache::{Checkouts, EnvironmentCache, OpenThread},
    completion::{
        BuiltinCommand, CompletionChoice, GitHubEntry, MentionEntry, SkillEntry, SlashEntry,
        available_commands, completion_choice_at, completion_count, completion_token_at,
        matching_commands, matching_github, matching_mentions, matching_skills,
    },
    composer_state::{
        AttachmentStatus, AttachmentTarget, CompletionKind, CompletionToken, ComposerState,
        DraftCursor, DraftData, DraftPill, PROMPT_CHAR_LIMIT, PROMPT_WARN_LIMIT, PaletteChoice,
        PaletteCommand, PaletteSearch, PaletteState,
    },
    home::{HomeProjectOption, draft_branch_options, home_project_options},
    model_list::{ListEmpty, ListItem, ModelRow, PickerList, picker_list},
    model_menu::{
        ACCESS_PICKER, Catalog, MODEL_PICKER, ModelMenuState, ModelRef, PickerColumn, PickerScope,
        RailTab, catalogs, picker_scope, status_note, status_usable,
    },
    model_options::{
        EffortColumn, EffortLevel, ExtraNav, ExtraOption, PickerNav, access_summary, access_view,
        context_badge, effort_column, picker_nav, speed,
    },
    palette::{PaletteRow, palette_rows},
    picker::{
        ModelLevels, ModelPicker, OptionRole, OptionTarget, choice_group, model_name, option_role,
        provider_name, sentence_label, thinking_ids, thinking_label, unseen_model_levels,
    },
    prefs::Prefs,
    sidebar::{
        ContextSubmenu, DragShelf, DropZone, InlineRename, ProjectFilter, SETTLED_PAGE_SIZE,
        SHELF_SETTLED, SHELF_SNOOZED, SHELF_WORKING, SidebarDrag, SnoozePreset, UndoTriage,
        apply_optimistic_triage, format_wake_label, sync_summary_derived,
    },
    sidebar_outline::{
        DraftPreview, ProjectFilterOption, SidebarOutline, nth_visible_thread,
        project_filter_options, sidebar_position,
    },
    state::{
        ConnectionState, ContextMenu, DIFF_DEFAULT_WIDTH, DIFF_MAX_WIDTH, DIFF_MIN_WIDTH,
        DisclosureState, NewThread, OptimisticLedger, OptimisticUndo, PaneSwitch, PanelsState,
        SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SelectionState, SessionId,
        SidebarEdit, SidebarState, Status, WINDOW_SIZE,
    },
    subagents::{
        AgentStanding, SubagentCall, child_link, delegated_task, local_child, spawned_subagent,
        subagent_call, subagent_title,
    },
    timeline::{
        FilePreview, FilePreviewBody, TimelineState, ToolAction, first_non_empty_line,
        humanize_tool, plural, relative_path, split_path,
    },
    tool_parse::is_command,
    transcript::{
        CompactionNotice, ErrorKind, Item, LoadedEntry, MediaItem, PlanBody, PlanCard, PlanStep,
        SearchMatch, TaskResult, Tool, ToolStatus, Transcript, TurnEnd, User,
    },
    turn_format::{
        diff_line_stats, format_clock_hm, format_cost, format_turn_duration, work_group_summary,
    },
    turns::{ChangedFile, TurnView, project_turns},
};

/// Default key used in per-environment maps before the server reports an
/// environment id in `hello`.
pub const DEFAULT_ENV_KEY: &str = "local";

/// Complete pure state of the desktop workspace.
#[derive(Debug, Clone)]
pub struct Model {
    pub requests: requests::Requests,
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
    /// Per-thread timeline fold memory, scroll tracking, and chunk
    /// loads.
    pub timeline: TimelineState,
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
            requests: requests::Requests::default(),
            palette: PaletteState::default(),
            disclosure: DisclosureState {
                tool_clock: now_ms,
                ..DisclosureState::default()
            },
            timeline: TimelineState::default(),
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

    /// Whether a collapsible timeline fold `key` on the active environment and
    /// thread is currently open.
    #[must_use]
    pub fn is_fold_open(&self, key: &str, default_open: bool) -> bool {
        self.timeline.is_open(
            self.active_env_key(),
            self.selection.selected,
            key,
            default_open,
        )
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

    /// Stable element key of the active timeline scroll viewport.
    #[must_use]
    pub fn timeline_page_key(&self) -> String {
        match (self.selection.selected, &self.selection.new_thread) {
            (Some(id), _) => format!("page-{id}"),
            (None, Some(draft)) => format!("page-draft-{}", draft.id),
            (None, None) => "page-none".to_owned(),
        }
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
        if self.composer.draft.text.chars().count() > PROMPT_CHAR_LIMIT
            || self.composer.draft.has_pending_or_failed_attachments()
        {
            return false;
        }
        let non_empty = !self.composer.draft.text.trim().is_empty()
            || !self.composer.draft.attachments.is_empty();
        if !non_empty {
            return false;
        }
        if self.composer.editing_queued.is_some() {
            return self.connected() && self.selection.selected.is_some();
        }
        let ready = if self.selection.new_thread.is_some() || self.selection.selected.is_some() {
            self.can_attach()
        } else {
            self.connected() && self.cache().has_projects()
        };
        ready
            && self.selection.new_thread.as_ref().is_none_or(|thread| {
                !thread.loading_choices && thread.active_harness(self.harnesses()).is_some()
            })
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
                            ..DraftData::default()
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
            sidebar_width: Some(self.panels.sidebar.width),
            sidebar_open: Some(self.panels.sidebar.open),
            diff_width: (self.panels.diff.width - DIFF_DEFAULT_WIDTH)
                .abs()
                .ge(&1.0)
                .then_some(self.panels.diff.width),
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
