//! Pure timeline derivations and client-side timeline UI state: tool action
//! classification, activity burst folding, per-thread fold memory,
//! scroll state, and path formatting.

use std::collections::{HashMap, HashSet};

use super::{Model, Tool, is_command};

/// Active file or attachment preview overlay state.
#[derive(Debug, Clone)]
pub struct FilePreview {
    /// Monotonic request serial used to ignore stale responses.
    pub request: u64,
    /// Display path or attachment filename.
    pub path: String,
    /// Optional 1-based inclusive line range (`start..=end`) highlighted and
    /// scrolled into view.
    pub lines: Option<(usize, usize)>,
    /// Optional external `file://` URL offered for opening in the OS handler.
    pub open_url: Option<String>,
    /// Loaded content or pending/error status.
    pub body: FilePreviewBody,
}

/// Content state of [`FilePreview`].
#[derive(Debug, Clone)]
pub enum FilePreviewBody {
    /// Waiting for `files.read` or a content-channel download.
    Loading,
    /// UTF-8 text or source code.
    Text {
        text: String,
        start_line: usize,
        truncated: bool,
        size: u64,
    },
    /// Raster or vector image decoded into `ShellWidgets::preview_image`.
    Image { size: u64 },
    /// Non-text binary file preview summary.
    Binary { mime: String, size: u64 },
    /// Error reading or decoding the file.
    Error(String),
}

/// Client-side timeline UI state persisted in session memory across thread
/// switches.
///
/// Per-thread disclosure overrides live in [`super::EnvState::folds`];
/// connection-ephemeral in-flight fetches live in [`super::LiveSession`].
#[derive(Debug, Clone, Default)]
pub struct TimelineState {
    /// Fallback disclosure map when no thread id is selected (e.g. draft).
    pub draft_folds: HashMap<String, bool>,
    /// Monotonic revision bumped whenever any disclosure toggle or dismissed
    /// banner changes, used to invalidate virtualized timeline projections.
    pub fold_rev: u64,
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
    /// Open plan card overflow menu key, if any.
    pub open_plan_menu: Option<String>,
    /// Dismissed inline error banner keys.
    pub dismissed_errors: HashSet<String>,
    /// File selected in the diff inspector.
    pub selected_diff_file: Option<String>,
    /// Open file or attachment preview overlay, if any.
    pub file_preview: Option<FilePreview>,
    /// Monotonic serial for [`Self::file_preview`] requests.
    pub file_preview_seq: u64,
}

impl TimelineState {
    /// Increments [`Self::fold_rev`] after a disclosure or visibility change.
    pub const fn bump_fold_rev(&mut self) {
        self.fold_rev = self.fold_rev.wrapping_add(1);
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
