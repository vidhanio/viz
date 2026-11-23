//! Pure timeline derivations and client-side timeline UI state: tool action
//! classification, activity burst folding, per-thread fold memory,
//! scroll state, and path formatting.

use std::collections::{BTreeMap, HashMap, HashSet};

use viz_client::protocol::ThreadId;

use super::{Model, Tool, is_command};

/// Client-side timeline state persisted in session memory across thread
/// switches.
#[derive(Debug, Clone, Default)]
pub struct TimelineState {
    /// Per-thread disclosure overrides (`true` = open, `false` = closed) for
    /// turn folds, tool inspectors, long user messages, plan cards, and changed
    /// file folders.
    pub folds_by_thread: BTreeMap<ThreadId, HashMap<String, bool>>,
    /// Fallback disclosure map when no thread id is selected (e.g. draft).
    pub draft_folds: HashMap<String, bool>,
    /// Markdown code-block keys (`"{msg_key}-code-{idx}"`) with word wrap
    /// enabled.
    pub wrapped_code: HashSet<String>,
    /// Whether the timeline viewport is scrolled > `120pt` above the bottom.
    pub far_from_bottom: bool,
    /// True when new streaming content arrived while scrolled away from bottom.
    pub unseen_activity: bool,
    /// Key of the most recently copied item (for transient Copy -> Check
    /// morph).
    pub copied_key: Option<String>,
    /// Entry keys currently fetching full data via `thread.entry_chunk`.
    pub loading_chunks: HashSet<String>,
    /// Open plan card overflow menu key, if any.
    pub open_plan_menu: Option<String>,
    /// Dismissed inline error banner keys.
    pub dismissed_errors: HashSet<String>,
    /// File selected in the diff inspector.
    pub selected_diff_file: Option<String>,
}

impl TimelineState {
    /// Reads an explicit fold override for `key` on `thread`, if recorded.
    #[must_use]
    pub fn fold_override(&self, thread: Option<ThreadId>, key: &str) -> Option<bool> {
        match thread {
            Some(id) => self.folds_by_thread.get(&id)?.get(key).copied(),
            None => self.draft_folds.get(key).copied(),
        }
    }

    /// Resolves whether a collapsible timeline element `key` is currently open,
    /// honoring per-thread overrides, legacy `disclosure.expanded` fixture
    /// keys, and `default_open`.
    #[must_use]
    pub fn is_open(
        &self,
        thread: Option<ThreadId>,
        disclosure: &HashSet<String>,
        key: &str,
        legacy_key: Option<&str>,
        default_open: bool,
    ) -> bool {
        if let Some(explicit) = self.fold_override(thread, key) {
            return explicit;
        }
        if disclosure.contains(key) || legacy_key.is_some_and(|k| disclosure.contains(k)) {
            return !default_open;
        }
        default_open
    }

    /// Sets an explicit open/closed state for `key` on `thread`.
    pub fn set_fold(&mut self, thread: Option<ThreadId>, key: String, open: bool) {
        match thread {
            Some(id) => {
                self.folds_by_thread
                    .entry(id)
                    .or_default()
                    .insert(key, open);
            }
            None => {
                self.draft_folds.insert(key, open);
            }
        }
    }

    /// Clears transient per-viewport state when switching threads while keeping
    /// per-thread fold memory intact.
    pub fn reset_viewport(&mut self) {
        self.far_from_bottom = false;
        self.unseen_activity = false;
        self.open_plan_menu = None;
        self.copied_key = None;
    }
}

/// Semantic category of a tool call for verbs, icons, and folded summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAction {
    /// Shell command execution.
    Command,
    /// File read.
    Read,
    /// File creation / write.
    Write,
    /// File edit / patch.
    Edit,
    /// File deletion.
    Delete,
    /// File move / rename.
    Move,
    /// Search / grep / glob.
    Search,
    /// Web fetch.
    Fetch,
    /// Any other tool.
    Other,
}

impl ToolAction {
    /// Classifies `tool` by its normalized name.
    #[must_use]
    pub fn of(tool: &Tool) -> Self {
        match tool.name.as_str() {
            name if is_command(name) => Self::Command,
            "read" | "read_file" => Self::Read,
            "write" | "create" | "write_file" => Self::Write,
            "edit" | "apply_patch" | "edit_file" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" | "grep" | "glob" | "web_search" => Self::Search,
            "fetch" | "web_fetch" => Self::Fetch,
            _ => Self::Other,
        }
    }

    /// Whether the tool's summary names a filesystem path.
    #[must_use]
    pub const fn on_file(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Write | Self::Edit | Self::Delete | Self::Move
        )
    }
}

/// Converts an unknown MCP or harness tool identifier into readable words.
#[must_use]
pub fn humanize_tool(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("mcp__")
        && let Some((server, tool)) = rest.split_once("__")
        && !server.is_empty()
        && !tool.is_empty()
    {
        let action = tool
            .split(['_', '-', ' ', '.'])
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        return format!("{server} · {action}");
    }
    let name = name.rsplit("__").next().unwrap_or(name);
    let mut words = name
        .split(['_', '-', ' ', '.'])
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(first) = words.get(..1) {
        let upper = first.to_uppercase();
        words.replace_range(..1, &upper);
    }
    words
}

/// Picks singular or plural noun form for `n`.
#[must_use]
pub const fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// Splits `path` into its directory prefix (with trailing slash) and filename.
#[must_use]
pub fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rfind('/')
        .map_or(("", trimmed), |cut| (&trimmed[..=cut], &trimmed[cut + 1..]))
}

/// Strips the open thread's working directory prefix from `path` when inside
/// it.
#[must_use]
pub fn relative_path<'a>(model: &Model, path: &'a str) -> &'a str {
    model
        .thread_state()
        .and_then(|state| path.strip_prefix(state.cwd.trim_end_matches('/')))
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
}

/// First non-empty trimmed line of `text`.
#[must_use]
pub fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}
