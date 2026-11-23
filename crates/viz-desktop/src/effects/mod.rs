//! Edge effects layer: the only code in `viz-desktop` that talks to the server
//! runtime, filesystem, clipboard, notifications, file dialogs, or preferences
//! file.

pub mod platform;
pub mod prefs;
pub mod requests;
pub mod server;

use std::{path::PathBuf, time::Duration};

use viz_client::protocol::{
    Attachment, EntryId, LaunchThread, OptionValue, SendInput, ThreadId, WorktreeTarget,
};

use crate::model::{AttachmentTarget, Prefs};

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
    /// Open the OS folder picker to add a project.
    PickProjectFolder,
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
}

/// Which shell-owned [`rune::TextEdit`] widget to synchronize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// Main composer prompt editor.
    Draft,
    /// Sidebar thread filter field.
    SidebarFilter,
    /// Sidebar rename modal input field.
    SidebarDialog,
    /// Command palette query input.
    PaletteQuery,
    /// Model picker search input.
    ModelQuery,
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
        params: LaunchThread,
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
    /// Update or triage an existing thread, paired with an optimistic `op_id`.
    Update {
        /// Optimistic mutation id for ack/rollback tracking.
        op_id: u64,
        /// Target thread id.
        thread: ThreadId,
        /// Requested thread mutation.
        change: ThreadChange,
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
    /// Add, rename, or remove a project.
    Project {
        /// Optional optimistic mutation id (`Some` for rename/remove).
        op_id: Option<u64>,
        /// Requested project mutation.
        change: ProjectChange,
    },
    /// Search project files for `@` completion.
    SearchFiles {
        /// Active thread id.
        thread: ThreadId,
        /// Monotonic completion request id.
        request: u64,
        /// File query string.
        query: String,
    },
    /// Full-text search across transcripts for the command palette.
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

/// Thread metadata or lifecycle change sent over RPC.
#[derive(Debug, Clone)]
pub enum ThreadChange {
    /// User rename.
    Title(String),
    /// Archive (`true`) or unarchive (`false`).
    Archive(bool),
    /// Pin (`true`) or unpin (`false`).
    Pin(bool),
    /// Mark unread (`true`) or read (`false`).
    Unread(bool),
    /// Reassign project path (or detach from project when `None`).
    Project(Option<String>),
}

/// Project record change sent over RPC.
#[derive(Debug, Clone)]
pub enum ProjectChange {
    /// Register an existing directory as a project.
    Add(String),
    /// Rename a registered project.
    Rename {
        /// Project path id.
        id: String,
        /// New display name.
        name: String,
    },
    /// Unregister a project without deleting files.
    Remove(String),
}
