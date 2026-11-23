//! Pure composer and command-palette state structs stored inside
//! [`crate::model::Model`].

use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

use viz_client::protocol::{Attachment, SearchHit, ThreadId};

/// Unsent composer text and prepared attachments for one thread or draft.
#[derive(Debug, Clone, Default)]
pub struct DraftData {
    /// Plain prompt text.
    pub text: String,
    /// Prepared attachments queued for the next send.
    pub attachments: Vec<Attachment>,
}

impl DraftData {
    /// Whether neither text nor attachments are present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.text.is_empty() && self.attachments.is_empty()
    }
}

/// Which composer target initiated an asynchronous attachment read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentTarget {
    /// Unsent local draft thread.
    Draft(u64),
    /// Existing server thread.
    Thread(ThreadId),
}

/// Kind of inline composer completion trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// Leading `/command` completion.
    Slash,
    /// Inline `@file` mention completion.
    File,
}

/// Parsed inline completion token at the composer caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionToken {
    /// Slash command or file mention.
    pub kind: CompletionKind,
    /// Byte range of the token inside the draft string.
    pub range: Range<usize>,
    /// Query text after the trigger character.
    pub query: String,
}

/// Pure snapshot of the composer editor cursor passed into completion logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftCursor {
    /// Whether the composer input has keyboard focus.
    pub focused: bool,
    /// Selection anchor byte offset.
    pub anchor: usize,
    /// Caret focus byte offset.
    pub focus: usize,
}

/// Pure composer completion drawer state.
#[derive(Debug, Clone, Default)]
pub struct CompletionState {
    /// Currently tracked token at the caret.
    pub token: Option<CompletionToken>,
    /// Monotonic request id matching asynchronous file-search replies.
    pub request: u64,
    /// Clock deadline for debounced file search.
    pub due: Option<f64>,
    /// Latest file-search outcome for `request`.
    pub files: Option<Result<Vec<String>, String>>,
    /// Highlighted row index.
    pub highlighted: usize,
    /// True after pressing Escape on the current token.
    pub dismissed: bool,
}

impl CompletionState {
    /// Resets completion state and invalidates any in-flight file search.
    pub fn reset(&mut self) {
        self.token = None;
        self.request = self.request.wrapping_add(1);
        self.due = None;
        self.files = None;
        self.highlighted = 0;
        self.dismissed = false;
    }

    /// Whether `token` matches the currently tracked completion token.
    #[must_use]
    pub fn matches(&self, token: &CompletionToken) -> bool {
        self.token.as_ref() == Some(token)
    }

    /// Seeds a slash-command completion state for preview scenes.
    pub fn preview_slash(&mut self, query: &str, highlighted: usize) {
        self.reset();
        self.token = Some(CompletionToken {
            kind: CompletionKind::Slash,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.highlighted = highlighted;
    }

    /// Seeds a file-mention completion state for preview scenes.
    pub fn preview_files(&mut self, query: &str, paths: Vec<String>, highlighted: usize) {
        self.reset();
        self.token = Some(CompletionToken {
            kind: CompletionKind::File,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.files = Some(Ok(paths));
        self.highlighted = highlighted;
    }
}

/// Pure state of the open model-and-thinking picker menu.
#[derive(Debug, Clone, Default)]
pub struct ModelMenuState {
    /// Search input text.
    pub query: String,
    /// Highlighted model value; current model or first match when unset.
    pub active: Option<String>,
    /// Optimistically shown thinking choice `(option_id, value)` while the menu
    /// stays open on this model.
    pub chosen: Option<(String, String)>,
    /// Model value whose thinking levels are being fetched.
    pub loading_levels: Option<String>,
}

/// Pure composer state: active draft, per-environment saved drafts, pickers,
/// completions, and model favorites.
#[derive(Debug, Clone, Default)]
pub struct ComposerState {
    /// Text and attachments currently in the composer.
    pub draft: DraftData,
    /// Whether the composer text field currently has keyboard focus.
    pub draft_focused: bool,
    /// Saved drafts keyed by environment id then thread id.
    pub drafts_by_env: BTreeMap<String, BTreeMap<ThreadId, DraftData>>,
    /// In-flight attachment reads preventing overlapping reads on one target.
    pub attachment_reads: HashSet<AttachmentTarget>,
    /// Slash and `@file` completion state.
    pub completion: CompletionState,
    /// Model picker popover state.
    pub model_menu: ModelMenuState,
    /// Key of whichever inline picker menu is currently open.
    pub open_picker: Option<String>,
    /// User-starred model identifiers persisted in client preferences.
    pub favorite_models: Vec<String>,
}

/// Local action executable from the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    /// Start a new draft thread.
    NewThread,
    /// Add a project directory.
    AddProject,
    /// Show or hide the sidebar.
    Sidebar,
    /// Pin or unpin the selected thread.
    Pin,
    /// Archive or unarchive the selected thread.
    Archive,
    /// Confirm deleting the selected thread.
    Delete,
    /// Abort the active turn.
    Stop,
}

/// Selectable row inside the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteChoice {
    /// A local desktop command.
    Command(PaletteCommand),
    /// Open an existing thread.
    Thread(ThreadId),
    /// Jump to a thread matched by transcript search.
    Message {
        /// Matched thread id.
        thread: ThreadId,
        /// Matched entry id.
        entry: u64,
    },
}

impl PaletteChoice {
    /// Stable element key for scrolling and accessibility active-descendant
    /// references.
    #[must_use]
    pub fn key(self) -> String {
        match self {
            Self::Command(command) => format!("palette-command-{command:?}"),
            Self::Thread(id) => format!("palette-thread-{id}"),
            Self::Message { thread, entry } => format!("palette-hit-{thread}-{entry}"),
        }
    }
}

/// Lifecycle of the command palette's server transcript search.
#[derive(Debug, Clone, Default)]
pub enum PaletteSearch {
    /// No query scheduled.
    #[default]
    Idle,
    /// Waiting for debounce deadline before querying the server.
    Debouncing(f64),
    /// Request in flight.
    Pending,
    /// Completed with hits or an error message.
    Complete(Result<Vec<SearchHit>, String>),
}

/// Pure command palette modal state.
#[derive(Debug, Clone, Default)]
pub struct PaletteState {
    /// Whether the modal owns keyboard input.
    pub open: bool,
    /// Raw query text typed into the search field.
    pub query: String,
    /// Trimmed lowercase filter derived from `query`.
    pub filter: String,
    /// Keyboard-highlighted choice, if explicitly moved.
    pub active: Option<PaletteChoice>,
    /// Monotonic request counter rejecting stale search replies.
    pub request: u64,
    /// Server transcript search phase.
    pub search: PaletteSearch,
}

impl PaletteState {
    /// Invalidates any pending transcript search.
    pub fn invalidate(&mut self) {
        self.request = self.request.wrapping_add(1);
        self.search = PaletteSearch::Idle;
    }

    /// Completed transcript search hits, or empty while pending/errored.
    #[must_use]
    pub fn hits(&self) -> &[SearchHit] {
        match &self.search {
            PaletteSearch::Complete(Ok(hits)) => hits,
            _ => &[],
        }
    }
}
