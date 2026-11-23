//! Edge effects layer: the only code in `viz-desktop` that talks to the server
//! runtime, filesystem, clipboard, notifications, file dialogs, or preferences
//! file.

mod composer_rpc;
pub mod git;
mod offline;
pub mod platform;
pub mod prefs;
pub mod projects;
pub mod requests;
mod runner;
pub mod server;
mod timeline_rpc;

use std::{path::PathBuf, time::Duration};

use viz_client::protocol::{
    Attachment, EntryId, EntrySource, ForkPoint, ForkThread, LaunchThread, OptionValue, RunId,
    SendInput, ThreadId, TriageAction, WorktreeTarget,
};

use crate::model::{AttachmentTarget, DraftPill, Prefs, UndoTriage};

/// Maximum base64-encoded bytes per message across all attachments.
pub const ATTACHMENT_ENCODED_LIMIT: usize = platform::ENCODED_LIMIT;

/// Pure description of an edge or shell side effect returned by
/// [`crate::update::update`].
#[derive(Debug)]
pub enum Effect {
    /// Send a command to the background server connection runtime.
    Server(ServerEffect),
    /// Load attachments on the blocking pool from file dialog, dropped paths,
    /// or pasted clipboard PNG bytes.
    Attach {
        /// Active environment epoch to ignore stale completions.
        scope: u64,
        /// Composer target receiving the loaded attachments.
        target: AttachmentTarget,
        /// Source of files or image bytes.
        source: AttachmentSource,
    },
    /// Deliver an OS desktop notification.
    Notify {
        /// Notification summary/title.
        title: String,
        /// Short body text.
        body: &'static str,
    },
    /// Persist client preferences to `$XDG_STATE_HOME/viz/desktop.json`.
    SavePrefs(Prefs),

    /// Copy plain text to the system clipboard.
    CopyText(String),
    /// Open an external URL in the default handler.
    OpenUrl(String),
    /// Update the OS window title.
    SetWindowTitle(&'static str),
    /// Show a floating toast notification.
    Toast {
        /// Primary one-line toast message.
        title: String,
        /// Optional multi-line detail.
        detail: Option<String>,
    },
    /// Show a floating undo toast with an inline `Undo` button restoring prior
    /// thread triage state.
    UndoToast {
        /// Short summary (e.g., `"Thread settled"`).
        title: String,
        /// Optional thread title detail.
        detail: Option<String>,
        /// Prior lifecycle restored when the user clicks `Undo`.
        undo: UndoTriage,
        /// Auto-dismiss lifetime in seconds.
        secs: f64,
    },
    /// Decode fetched project icon image bytes in the shell's image cache.
    DecodeProjectIcon {
        /// Environment id owning this project.
        environment: String,
        /// Project path id.
        project: String,
        /// MIME media type (raster or `image/svg+xml`).
        media_type: String,
        /// Decoded raw image bytes.
        bytes: Vec<u8>,
    },
    /// Move keyboard focus to element `key`.
    Focus {
        /// Element key to focus.
        key: &'static str,
        /// Whether to select text when focusing.
        select_all: bool,
        /// Defer focus until the element mounts.
        when_shown: bool,
    },
    /// Scroll a scrollable container to its bottom edge.
    ScrollToBottom(&'static str),
    /// Jump a scrollable container immediately to `(x, y)`.
    ScrollToInstant {
        /// Scroll container element key.
        key: &'static str,
        /// Horizontal scroll offset.
        x: f32,
        /// Vertical scroll offset.
        y: f32,
    },
    /// Ensure item `item` is visible inside list `list`.
    ScrollIntoView {
        /// Scroll container key.
        list: &'static str,
        /// Target item key.
        item: String,
    },
    /// Center item `item` immediately inside list `list`.
    ScrollCenterInstant {
        /// Scroll container key.
        list: &'static str,
        /// Target item key.
        item: String,
    },
    /// Scroll the enclosing viewport so `key`'s vertical midpoint sits at
    /// `center_y` points below the viewport's top.
    ScrollKeyCenterAt {
        /// Target element key.
        key: String,
        /// Vertical center position in viewport points.
        center_y: f32,
    },
    /// Center a matched transcript row in the active thread.
    RevealMessage { thread: ThreadId, item: String },
    /// Anchor element `key` in place across the next layout update.
    KeepOnScreen(String),
    /// Schedule a poll wakeup after `duration`.
    WakeAfter(Duration),
    /// Synchronize a shell-owned Rune [`rune::TextEdit`] widget after a pure
    /// state update changed or replaced its contents.
    SyncEdit(EditSync),
    /// Synchronize the shell-owned sidebar [`rune_kit::shell::PaneState`].
    SyncSidebarPane {
        /// Set open/closed state when `Some`.
        open: Option<bool>,
        /// Set width in points when `Some`.
        width: Option<f32>,
    },
    /// Synchronize the shell-owned diff inspector
    /// [`rune_kit::shell::PaneState`].
    SyncDiffPane {
        /// Set open/closed state when `Some`.
        open: Option<bool>,
        /// Set width in points when `Some`.
        width: Option<f32>,
    },
    /// Expand and scroll to `path` inside the docked Diff panel.
    RevealDiffFile(String),
}

/// Which shell-owned [`rune::TextEdit`] widget to synchronize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// Main composer prompt editor.
    Draft,
    /// Project filter combobox search input.
    ProjectFilter,
    /// Sidebar inline thread title rename input.
    InlineRename,
    /// Sidebar rename modal input field.
    SidebarDialog,
    /// Command palette query input.
    PaletteQuery,
    /// Model picker search input.
    ModelQuery,
    /// Home hero project picker search input.
    HomeProjectQuery,
    /// Home context strip branch picker search input.
    DraftBranchQuery,
}

/// Mutation applied to a shell-owned [`rune::TextEdit`] widget.
#[derive(Debug, Clone)]
pub enum EditSync {
    /// Replace text contents and clear selection.
    Replace {
        /// Target input widget.
        target: EditTarget,
        /// New text value.
        text: String,
        /// Select the entire replaced string when true.
        select_all: bool,
    },
    /// Replace the composer draft editor with text plus atomic inline pills.
    ReplaceDraftWithPills {
        /// Plain prompt text including pill label spans.
        text: String,
        /// Atomic pills anchored to byte ranges in `text`.
        pills: Vec<DraftPill>,
    },
    /// Clear text contents.
    Clear(EditTarget),
    /// Prepend restored message text to the active draft when a send fails.
    PrependDraft(String),
    /// Replace a byte range in the draft with an accepted completion token.
    ReplaceDraftRange {
        /// Start byte offset.
        start: usize,
        /// End byte offset.
        end: usize,
        /// Replacement text.
        insertion: String,
    },
    /// Replace a byte range in the draft with an atomic [`rune::Mark::Pill`].
    InsertDraftPill {
        /// Start byte offset of the trigger token.
        start: usize,
        /// End byte offset of the trigger token.
        end: usize,
        /// Typed pill id.
        id: String,
        /// Visible inline label.
        label: String,
        /// Append a trailing space after the pill when true.
        trailing_space: bool,
    },
}

/// Source for loading attachments asynchronously.
#[derive(Debug, Clone)]
pub enum AttachmentSource {
    /// Open the OS multi-file picker.
    Picker,
    /// Read explicit local file paths (from drag-and-drop or clipboard files).
    Files(Vec<PathBuf>),
    /// Encode raw PNG image bytes pasted from the clipboard.
    ClipboardPng(Vec<u8>),
}

/// Result of reading a batch of attachments on the blocking pool.
#[derive(Debug, Clone, Default)]
pub struct LoadedAttachments {
    /// Encoded attachments ready to attach.
    pub attachments: Vec<Attachment>,
    /// Per-file errors to report via toast.
    pub errors: Vec<String>,
}

/// Command sent to the background server connection task.
#[derive(Debug)]
pub enum ServerEffect {
    Answer(viz_client::protocol::AnswerRequest),
    Git {
        thread: ThreadId,
        serial: u64,
        request: git::Request,
    },
    /// Watch this thread instead.
    Select(ThreadId),
    /// Stop watching the active thread.
    ClearSelection,
    /// Reconnect against another environment, optionally opening `thread`.
    Environment {
        /// Target environment id.
        environment: String,
        /// Optional thread to open once connected.
        thread: Option<ThreadId>,
    },
    /// Cancel a delegated task.
    CancelTask(String),
    /// Launch a new thread from a fresh draft and send its first prompt.
    Create {
        /// Local draft sequence id.
        draft: u64,
        /// Launch parameters.
        params: Box<LaunchThread>,
    },
    /// Fetch thinking levels for a specific `(harness, model)` pair.
    HarnessOptions {
        /// Harness configuration id.
        harness: String,
        /// Selected model value.
        model: String,
    },
    /// Refresh harness choices for a fresh draft.
    LoadDraft(u64),
    /// Refresh the harness catalog in the background (model picker open).
    Harnesses,
    /// Choose a thread's access mode; `None` returns it to the project default.
    Access {
        /// Target thread id.
        thread: ThreadId,
        /// Mode to apply.
        access: Option<viz_client::protocol::AccessMode>,
    },
    /// Update or triage an existing thread, paired with an optimistic `op_id`.
    Update {
        /// Optimistic mutation id for ack/rollback tracking.
        op_id: u64,
        /// Target thread id.
        thread: ThreadId,
        /// Requested thread mutation.
        change: ThreadChange,
    },
    /// Regenerate a thread's title from the harness or conversation.
    Retitle(ThreadId),
    /// Read a project's image icon if its `ETag` changed.
    ReadProjectIcon {
        /// Project path id.
        project: String,
        /// Previously cached `ETag`, if any.
        etag: Option<String>,
    },
    /// Delete a thread and optionally its managed worktree.
    Delete {
        /// Target thread id.
        thread: ThreadId,
        /// Remove the managed Git worktree when true.
        remove_worktree: bool,
    },
    /// Load branches and worktrees for `project`.
    LoadCheckouts(String),
    /// Move an idle thread to another Git worktree.
    Move {
        /// Target thread id.
        thread: ThreadId,
        /// Target checkout.
        worktree: WorktreeTarget,
    },
    /// Apply an ACP config option on an active thread.
    Configure {
        /// Target thread id.
        thread: ThreadId,
        /// Option identifier.
        option: String,
        /// New option value.
        value: OptionValue,
    },
    /// Stop an active run on a thread.
    Abort(ThreadId),
    /// Update a project pin.
    Project {
        /// Optional optimistic mutation id.
        op_id: Option<u64>,
        /// Requested project mutation.
        change: ProjectChange,
    },
    /// Search project files for `@` completion.
    SearchFiles {
        /// Active thread whose checkout is searched.
        thread: Option<ThreadId>,
        /// Project directory searched when composing a new thread.
        project: Option<String>,
        /// Monotonic completion request id.
        request: u64,
        /// File query string.
        query: String,
    },
    /// Search GitHub issues and pull requests for `#` completion.
    SearchGitHub {
        /// Active project path.
        project: String,
        /// Monotonic completion request id.
        request: u64,
        /// Issue/PR query string.
        query: String,
    },
    /// Promote a queued input to steer immediately.
    SteerQueued {
        /// Target thread id.
        thread: ThreadId,
        /// Queued run id.
        run: RunId,
    },
    /// Update a queued input's prompt text.
    UpdateQueued {
        /// Target thread id.
        thread: ThreadId,
        /// Queued run id.
        run: RunId,
        /// Updated prompt text.
        text: String,
    },
    /// Remove a queued input from the queue.
    RemoveQueued {
        /// Target thread id.
        thread: ThreadId,
        /// Queued run id.
        run: RunId,
    },
    /// Full-text search across transcripts for the command palette.
    ProjectPalette(u64, projects::ProjectRequest),
    SearchThreads {
        /// Monotonic palette request id.
        request: u64,
        /// Search query string.
        query: String,
    },
    /// Fetch an older page of entries for `thread`.
    LoadEarlier {
        /// Target thread id.
        thread: ThreadId,
        /// Exclusive entry cursor.
        before: EntryId,
    },
    /// Send a prompt or steering message to an existing thread.
    SendInput(SendInput),
    /// Fork a thread from a specific transcript point.
    Fork {
        /// Fork parameters.
        params: ForkThread,
        /// Whether to populate the new thread's composer with the excluded
        /// prompt.
        load_prompt: bool,
    },
    /// Lazily download a truncated or omitted transcript entry in chunks.
    ReadEntryChunk {
        /// Thread id owning the entry.
        thread: ThreadId,
        /// Stable UI key for clearing loading state.
        key: String,
        /// Stored entry id or live key selector.
        source: EntrySource,
    },
    /// Reconnect immediately instead of waiting for the retry timer.
    Reconnect,
    /// Load attachments on the runtime's blocking pool.
    Attach {
        /// Environment epoch.
        scope: u64,
        /// Composer target.
        target: AttachmentTarget,
        /// Attachment source.
        source: AttachmentSource,
    },
    /// Deliver a desktop notification on the runtime.
    Notify {
        /// Notification summary.
        title: String,
        /// Short body string.
        body: &'static str,
    },
}

impl ServerEffect {
    /// Builds a [`ServerEffect::Fork`] cloning `thread` from its head.
    #[must_use]
    pub const fn fork_head(thread: ThreadId) -> Self {
        Self::Fork {
            params: ForkThread {
                thread,
                point: ForkPoint::Head,
                worktree: None,
                harness: None,
                title: None,
                request_id: None,
            },
            load_prompt: false,
        }
    }
}

/// Thread metadata or lifecycle change sent over RPC.
#[derive(Debug, Clone)]
pub enum ThreadChange {
    /// User rename.
    Title(String),
    /// Lifecycle triage mutation (`thread.triage`).
    Triage(TriageAction),
    /// Reassign project path (or detach from project when `None`).
    Project(Option<String>),
}

/// Project record change sent over RPC.
#[derive(Debug, Clone)]
pub enum ProjectChange {
    /// Pin or unpin a registered project.
    Pin {
        /// Project path id.
        id: String,
        /// New pinned flag.
        pinned: bool,
    },
}
