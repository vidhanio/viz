//! Pure composer, inline pill, completion drawer, home picker, and
//! command-palette state structs stored inside [`crate::model::Model`].

use std::ops::Range;

use viz_client::protocol::{FileSearchMatch, GitHubItem, PromptRef, RunId, SearchHit, ThreadId};

use crate::composer::codec::{DraftPill, PillRef, RefId};

/// Local unsent or fetched raw-byte attachment held in memory without base64
/// encoding.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftAttachment {
    pub id: RefId,
    /// Display filename.
    pub name: String,
    /// MIME media type.
    pub mime: String,
    /// Raw unencoded file bytes.
    pub bytes: Vec<u8>,
}

impl DraftAttachment {
    /// References a local attachment until its bytes have been staged.
    pub fn pill(&self) -> (PillRef, String) {
        let target = PromptRef::Attachment {
            id: self.id.to_string(),
            name: self.name.clone(),
            mime: self.mime.clone(),
        };
        (
            PillRef::with_id(self.id, target),
            self.name.replace(['\n', '\r'], " "),
        )
    }
}

impl std::fmt::Debug for DraftAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftAttachment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("mime", &self.mime)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

/// Maximum prompt characters before warning and disabling Send.
pub const PROMPT_CHAR_LIMIT: usize = 200_000;
/// Soft character threshold showing a non-blocking length caution badge.
pub const PROMPT_WARN_LIMIT: usize = 160_000;

/// Upload / decoding status of an attached composer file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentStatus {
    /// Ready to send.
    Ready,
    /// Upload or encoding progress (`0..=100`, `None` for indeterminate).
    Uploading(Option<u8>),
    /// Failed with a user-readable error message.
    Error(String),
}

/// Unsent composer text, atomic pills, and prepared attachments for one thread
/// or draft.
#[derive(Debug, Clone, Default)]
pub struct DraftData {
    /// Submission key and payload fingerprint retained after an uncertain
    /// reply.
    pub retry_submission: Option<(String, [u8; 32])>,
    /// Markdown prompt text (including pill label spans).
    pub text: String,
    /// Atomic inline chips synced with `ShellWidgets::draft`.
    pub pills: Vec<DraftPill>,
    /// Prepared attachments queued for the next send.
    pub attachments: std::sync::Arc<Vec<DraftAttachment>>,
    /// Per-attachment transient status (defaults to `Ready` when shorter than
    /// `attachments`).
    pub attachment_statuses: Vec<AttachmentStatus>,
}

impl DraftData {
    /// Whether neither text nor attachments are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.attachments.is_empty()
    }

    /// Status for attachment `index`, defaulting to
    /// [`AttachmentStatus::Ready`].
    #[must_use]
    pub fn attachment_status(&self, index: usize) -> &AttachmentStatus {
        self.attachment_statuses
            .get(index)
            .unwrap_or(&AttachmentStatus::Ready)
    }

    /// Whether any attachment is currently uploading or failed.
    #[must_use]
    pub fn has_pending_or_failed_attachments(&self) -> bool {
        self.attachment_statuses
            .iter()
            .any(|status| !matches!(status, AttachmentStatus::Ready))
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
    /// `/command` completion (built-in actions + ACP harness commands).
    Slash,
    /// `@` mention completion (files, threads, environments, models).
    Mention,
    /// `$` skill completion (ignoring numeric currency like `$100`).
    Skill,
    /// `#` GitHub issue and pull request mention completion.
    PullRequest,
}

/// Parsed inline completion token at the composer caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionToken {
    /// Trigger kind.
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
    /// Monotonic request id matching asynchronous file and GitHub search
    /// replies.
    pub request: u64,
    /// Clock deadline for debounced server search.
    pub due: Option<f64>,
    /// Latest ranked file-search outcome for `request`.
    pub files: Option<Result<Vec<FileSearchMatch>, String>>,
    /// Query string that produced [`Self::files`], used to keep instant
    /// client-side filtered results visible while a fresh search runs.
    pub files_query: Option<String>,
    /// Latest GitHub issue/PR search outcome for `request`.
    pub github: Option<Result<Vec<GitHubItem>, String>>,
    /// Highlighted row index.
    pub highlighted: usize,
    /// True after pressing Escape on the current token.
    pub dismissed: bool,
}

impl CompletionState {
    /// Resets completion state and invalidates any in-flight search.
    pub fn reset(&mut self) {
        self.token = None;
        self.request = self.request.wrapping_add(1);
        self.due = None;
        self.files = None;
        self.files_query = None;
        self.github = None;
        self.highlighted = 0;
        self.dismissed = false;
    }

    /// Updates the tracked token while keeping previous results of the same
    /// [`CompletionKind`] visible so typing never flashes empty.
    pub fn advance(&mut self, token: CompletionToken) {
        let same_kind = self
            .token
            .as_ref()
            .is_some_and(|prev| prev.kind == token.kind);
        if same_kind {
            self.token = Some(token);
            self.request = self.request.wrapping_add(1);
            self.due = None;
            self.highlighted = 0;
            self.dismissed = false;
        } else {
            self.reset();
            self.token = Some(token);
        }
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
    pub fn preview_files(&mut self, query: &str, files: Vec<FileSearchMatch>, highlighted: usize) {
        self.reset();
        self.token = Some(CompletionToken {
            kind: CompletionKind::Mention,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.files = Some(Ok(files));
        self.highlighted = highlighted;
    }

    /// Seeds a `$` skill completion state for preview scenes.
    pub fn preview_skills(&mut self, query: &str, highlighted: usize) {
        self.reset();
        self.token = Some(CompletionToken {
            kind: CompletionKind::Skill,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.highlighted = highlighted;
    }

    /// Seeds a `#` GitHub PR/issue completion state for preview scenes.
    pub fn preview_github(&mut self, query: &str, items: Vec<GitHubItem>, highlighted: usize) {
        self.reset();
        self.token = Some(CompletionToken {
            kind: CompletionKind::PullRequest,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.github = Some(Ok(items));
        self.highlighted = highlighted;
    }
}

/// Pure state of the Home hero searchable Project Picker combobox.
#[derive(Debug, Clone, Default)]
pub struct HomeProjectPickerState {
    /// Search text inside the Home project picker.
    pub query: String,
    /// Keyboard-highlighted row index.
    pub highlighted: usize,
}

/// Pure state of the Home context strip branch search combobox.
#[derive(Debug, Clone, Default)]
pub struct DraftBranchPickerState {
    /// Search text inside the draft branch picker.
    pub query: String,
    /// Keyboard-highlighted row index.
    pub highlighted: usize,
}

/// Pure composer state: active draft, pickers, completions, and model
/// favorites.
///
/// Per-environment saved drafts and prompt recall history live in
/// [`super::EnvState`]; in-flight attachment reads live in
/// [`super::LiveSession`].
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent composer UI state flags"
)]
pub struct ComposerState {
    /// Text, inline pills, and attachments currently in the composer.
    pub draft: DraftData,
    /// Whether the composer text field currently has keyboard focus.
    pub draft_focused: bool,
    /// True while files are being dragged over the composer drop zone.
    pub drag_over: bool,
    /// Index of the image attachment open in the full-size lightbox preview.
    pub lightbox: Option<usize>,
    /// Slash, `@`, `$`, and `#` completion state.
    pub completion: CompletionState,
    /// Home project picker combobox search and highlight state.
    pub home_project: HomeProjectPickerState,
    /// Home context strip branch picker search and highlight state.
    pub draft_branch: DraftBranchPickerState,
    /// Model picker popover state.
    pub model_menu: super::ModelMenuState,
    /// Key of whichever inline picker menu is currently open.
    pub open_picker: Option<String>,
    /// User-starred model identifiers persisted in client preferences.
    pub favorite_models: Vec<String>,
    /// Active index while walking prompt history (`0` = oldest).
    pub history_cursor: Option<usize>,
    /// Unsent draft stashed when entering prompt history recall.
    pub history_stash: Option<DraftData>,
    /// Queued run currently loaded into the composer for inline editing.
    pub editing_queued: Option<RunId>,
    /// Draft stashed while editing a queued message.
    pub editing_stash: Option<DraftData>,
    /// Whether queued messages past the first 3 are expanded.
    pub queued_expanded: bool,
    /// Optional compact-before-send toggle when context usage is high.
    pub compact_before_send: bool,
    /// Clock deadline (`s`) until which Stop click guard suppresses accidental
    /// double-presses.
    pub stop_guard_until: Option<f64>,
}

/// Local action executable from the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteCommand {
    /// Start a new draft thread.
    NewThread,
    /// Add a project directory.
    AddProject,
    Projects,
    Settings,
    Import,
    Promote,
    Environments,
    Diff,
    Scratch,
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
    Project(usize),
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
            Self::Project(index) => format!("palette-project-{index}"),
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
    pub pages: super::projects::ProjectPages,
    pub message_jump: Option<(ThreadId, viz_client::protocol::EntryId)>,
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
