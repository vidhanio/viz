//! Plain client-side state structs stored inside [`crate::model::Model`].
//!
//! Zero Rune types, channels, or I/O handles live here.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use viz_client::protocol::{
    AccessMode, Environment, Harness, OptionValue, Project, ThreadId, ThreadSummary, WorktreeTarget,
};

use super::{
    prefs::default_collapsed_shelves,
    sidebar::{ContextSubmenu, InlineRename, ProjectFilter, SETTLED_PAGE_SIZE, SidebarDrag},
};

/// The window's default size on launch, in points; headless renders use it too.
pub const WINDOW_SIZE: (f32, f32) = (1240.0, 760.0);
/// Default sidebar pane width in points.
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 264.0;
/// Minimum resizable sidebar width in points.
pub const SIDEBAR_MIN_WIDTH: f32 = 220.0;
/// Maximum resizable sidebar width in points.
pub const SIDEBAR_MAX_WIDTH: f32 = 440.0;
/// Default docked diff inspector width in points.
pub const DIFF_DEFAULT_WIDTH: f32 = 480.0;
/// Minimum docked diff inspector width in points.
pub const DIFF_MIN_WIDTH: f32 = 320.0;
/// Maximum docked diff inspector width in points.
pub const DIFF_MAX_WIDTH: f32 = 840.0;
/// Default docked terminal drawer height in points.
pub const TERMINAL_DEFAULT_HEIGHT: f32 = 240.0;

/// The server connection status shown in the sidebar footer and empty state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Trying to connect.
    Connecting,
    /// Connected to a server of this version.
    Connected {
        /// Server version string.
        version: String,
    },
    /// Not connected, and why.
    Error(String),
}

/// Direction the main pane and toolbar title move when switching threads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PaneSwitch {
    /// Instant cut without spatial travel.
    #[default]
    Cut,
    /// Moving to a thread higher in the sidebar.
    Up,
    /// Moving to a thread lower in the sidebar.
    Down,
    /// Drilling into a delegated child thread.
    Into,
    /// Returning out to a parent thread.
    Out,
    /// Cross-fade (drafts or unlisted targets).
    Fade,
}

/// Connection metadata shared across the desktop session.
#[derive(Debug, Clone)]
pub struct ConnectionState {
    /// Live WebSocket connection status.
    pub status: Status,
    /// Display endpoint string, once desktop config is read.
    pub endpoint: Option<String>,
    /// Active environment identifier, once `hello` completes or an environment
    /// switch starts.
    pub environment: Option<String>,
    /// Known environments advertised by the server.
    pub environments: Vec<Environment>,
    /// Monotonic counter incremented on environment switches to discard stale
    /// background attachment reads.
    pub epoch: u64,
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self {
            status: Status::Connecting,
            endpoint: None,
            environment: None,
            environments: Vec::new(),
            epoch: 0,
        }
    }
}

/// A local draft thread whose ACP session opens on its first send.
#[derive(Debug, Clone)]
pub struct NewThread {
    /// Local monotonic draft id.
    pub id: u64,
    /// Chosen project path, if any.
    pub project: Option<String>,
    /// Which checkout of a Git project to run in; `None` is the project's own.
    pub worktree: Option<WorktreeTarget>,
    /// Whether `New worktree` bases on the remote-tracking ref.
    pub remote_base: bool,
    /// Explicitly chosen harness id; `None` uses the server default.
    pub harness: Option<String>,
    /// Initial ACP config option overrides.
    pub options: BTreeMap<String, OptionValue>,
    /// Access mode chosen before launch; `None` is the harness default.
    pub access: Option<AccessMode>,
    /// True while the initial launch request is in flight.
    pub sending: bool,
    /// True while refreshing harness choices for this draft.
    pub loading_choices: bool,
}

impl NewThread {
    /// Starts a new local draft targeting `project`, if any.
    #[must_use]
    pub fn new(id: u64, project: Option<&Project>) -> Self {
        Self {
            id,
            project: project.map(|project| project.path.clone()),
            worktree: None,
            remote_base: false,
            harness: None,
            options: BTreeMap::new(),
            access: None,
            sending: false,
            loading_choices: false,
        }
    }

    /// Updates the draft's project and clears any stale worktree selection.
    pub fn set_project(&mut self, project: Option<&Project>) {
        self.project = project.map(|project| project.path.clone());
        self.worktree = None;
        self.remote_base = false;
    }

    /// Resolves the active harness for this draft from `runners`.
    #[must_use]
    pub fn active_harness<'a>(&self, runners: &'a [Harness]) -> Option<&'a Harness> {
        runners.iter().find(|runner| {
            self.harness
                .as_deref()
                .map_or(runner.default, |id| id == runner.id)
        })
    }
}

/// Active thread, draft, switch direction, and back/forward navigation stacks.
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// Currently open thread in the main pane.
    pub selected: Option<ThreadId>,
    /// Active unsent new-thread draft, mutually exclusive with `selected`.
    pub new_thread: Option<NewThread>,
    /// How the most recent thread change moves the main pane and title.
    pub switch: PaneSwitch,
    /// Next local draft sequence id.
    pub next_draft: u64,
    /// Threads left by navigating, newest last, for back navigation.
    pub back: Vec<ThreadId>,
    /// Threads popped by going back, newest last, for forward navigation.
    pub forward: Vec<ThreadId>,
}

/// Geometry and open state of a resizable split pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneLayout {
    /// Whether the pane is currently expanded.
    pub open: bool,
    /// Current width in points.
    pub width: f32,
    /// Minimum width in points.
    pub min: f32,
    /// Maximum width in points.
    pub max: f32,
}

/// Visibility and sizing of every shell slot (`sidebar`, `details`, `diff`,
/// `terminal`) plus the current window width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelsState {
    /// Leading workspace sidebar slot.
    pub sidebar: PaneLayout,
    /// Trailing thread details card slot (populated in Phase 1 Track G).
    pub details_open: bool,
    /// Trailing docked diff inspector slot (populated in Phase 1 Track G).
    pub diff: PaneLayout,
    /// Bottom docked terminal drawer slot (populated in Phase 1 Track X).
    pub terminal_open: bool,
    /// Height of the bottom terminal drawer in points.
    pub terminal_height: f32,
    /// Last drawn window width in points, used for responsive layout choices.
    pub window_width: f32,
    pub window_height: f32,
}

impl Default for PanelsState {
    fn default() -> Self {
        Self {
            sidebar: PaneLayout {
                open: true,
                width: SIDEBAR_DEFAULT_WIDTH,
                min: SIDEBAR_MIN_WIDTH,
                max: SIDEBAR_MAX_WIDTH,
            },
            details_open: false,
            diff: PaneLayout {
                open: false,
                width: DIFF_DEFAULT_WIDTH,
                min: DIFF_MIN_WIDTH,
                max: DIFF_MAX_WIDTH,
            },
            terminal_open: false,
            terminal_height: TERMINAL_DEFAULT_HEIGHT,
            window_width: WINDOW_SIZE.0,
            window_height: WINDOW_SIZE.1,
        }
    }
}

/// Open context menu state for a sidebar thread card.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    /// Row element key (`thread-<id>`).
    pub key: String,
    /// Pointer coordinates when opened via secondary press.
    pub point: Option<(f32, f32)>,
    /// Currently expanded submenu, if any.
    pub submenu: Option<ContextSubmenu>,
}

impl ContextMenu {
    /// Creates a menu anchored at pointer position `(x, y)`.
    #[must_use]
    pub fn at(key: impl Into<String>, x: f32, y: f32) -> Self {
        Self {
            key: key.into(),
            point: Some((x, y)),
            submenu: None,
        }
    }

    /// Creates a context menu with `submenu` expanded for scene previews.
    #[must_use]
    pub fn preview(
        key: impl Into<String>,
        x: f32,
        y: f32,
        submenu: Option<ContextSubmenu>,
    ) -> Self {
        Self {
            key: key.into(),
            point: Some((x, y)),
            submenu,
        }
    }
}

/// Modal rename or confirmation edit initiated from the sidebar or palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarEdit {
    /// Confirm deleting a thread (keeping its worktree, if any).
    DeleteThread(ThreadId),
    /// Confirm deleting a thread together with its managed worktree.
    DeleteWithWorktree(ThreadId),
}

/// Pure sidebar UI state: project filter, triage shelf collapses, hover card,
/// inline rename, drag-and-drop state, popovers, menus, and dialogs.
#[derive(Debug, Clone)]
pub struct SidebarState {
    /// Active project filter (`All`, one project path, or `NoProject`).
    pub project_filter: ProjectFilter,
    /// Whether the project filter combobox popup is open.
    pub filter_open: bool,
    /// Search query typed inside the project filter combobox.
    pub filter_query: String,
    /// Highlighted row index inside the project filter combobox.
    pub filter_active: usize,
    /// Parent thread ids whose delegated child subtrees are collapsed.
    pub collapsed_parents: HashSet<ThreadId>,
    /// Shelf keys collapsed in the triage list (`working`, `snoozed`,
    /// `settled`).
    pub collapsed_shelves: BTreeSet<String>,
    /// Maximum number of settled threads currently shown in the Settled shelf.
    pub settled_limit: usize,
    /// Thread currently displaying the delayed right-anchored hover preview
    /// card.
    pub hover_card: Option<ThreadId>,
    /// Thread whose hover quick-action snooze preset popover is open.
    pub snooze_popover: Option<ThreadId>,
    /// Active inline title rename state on a thread card.
    pub inline_rename: Option<InlineRename>,
    /// Active pointer drag state for pinning and reordering cards.
    pub drag: Option<SidebarDrag>,
    /// Whether the footer environment switcher menu is open.
    pub env_menu: bool,
    /// Whether the footer read-only workspace/shortcuts popover is open.
    pub info_menu: bool,
    /// Open row context menu, if any.
    pub menu: Option<ContextMenu>,
    /// Active confirmation or rename dialog, if any.
    pub edit: Option<SidebarEdit>,
    /// Current text inside the active rename dialog input.
    pub edit_value: String,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            project_filter: ProjectFilter::All,
            filter_open: false,
            filter_query: String::new(),
            filter_active: 0,
            collapsed_parents: HashSet::new(),
            collapsed_shelves: default_collapsed_shelves(),
            settled_limit: SETTLED_PAGE_SIZE,
            hover_card: None,
            snooze_popover: None,
            inline_rename: None,
            drag: None,
            env_menu: false,
            info_menu: false,
            menu: None,
            edit: None,
            edit_value: String::new(),
        }
    }
}

impl SidebarState {
    /// Whether the collapsible shelf identified by `key` is currently open.
    #[must_use]
    pub fn is_shelf_open(&self, key: &str) -> bool {
        !self.collapsed_shelves.contains(key)
    }

    /// Closes all sidebar popovers, hover cards, inline edits, drags, and modal
    /// dialogs.
    pub fn dismiss_overlays(&mut self) {
        self.filter_open = false;
        self.filter_query.clear();
        self.filter_active = 0;
        self.hover_card = None;
        self.snooze_popover = None;
        self.inline_rename = None;
        self.drag = None;
        self.env_menu = false;
        self.info_menu = false;
        self.menu = None;
        self.edit = None;
    }
}

/// Disclosure toggles, tool timeout ticker, and window focus tracking.
#[derive(Debug, Clone)]
pub struct DisclosureState {
    /// Expanded timeline rows and tucked decks, by stable key.
    pub expanded: HashSet<String>,
    /// Wall-clock timestamp in Unix ms for running tool timeout rings.
    pub tool_clock: i64,
    /// Whether the desktop window currently has OS focus.
    pub focused: bool,
}

impl Default for DisclosureState {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            tool_clock: 0,
            focused: true,
        }
    }
}

/// Undo snapshot recorded before an optimistic cache mutation.
#[derive(Debug, Clone)]
pub enum OptimisticUndo {
    /// Restores a thread summary's prior fields after a failed update/triage.
    ThreadSummary {
        /// Environment cache key where the mutation ran.
        env: String,
        /// Thread id.
        id: ThreadId,
        /// Snapshot of the summary prior to optimistic mutation.
        prior: Box<ThreadSummary>,
    },
    /// Restores a project's prior pinned state after a failed pin toggle.
    ProjectPin {
        /// Environment cache key.
        env: String,
        /// Project path.
        path: String,
        /// Pinned flag before toggle.
        prior_pinned: bool,
    },
}

/// Tracks in-flight optimistic edits so server failures roll back cleanly.
#[derive(Debug, Clone, Default)]
pub struct OptimisticLedger {
    next_id: u64,
    pending: BTreeMap<u64, OptimisticUndo>,
}

impl OptimisticLedger {
    /// Records an undo snapshot and returns its unique mutation id.
    pub fn record(&mut self, undo: OptimisticUndo) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        self.pending.insert(id, undo);
        id
    }

    /// Drops the undo snapshot once the server acknowledges the mutation.
    pub fn commit(&mut self, id: u64) {
        self.pending.remove(&id);
    }

    /// Takes the undo snapshot when the server rejects the mutation.
    pub fn take(&mut self, id: u64) -> Option<OptimisticUndo> {
        self.pending.remove(&id)
    }

    /// Clears all pending rollbacks (e.g., on environment switch).
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}
