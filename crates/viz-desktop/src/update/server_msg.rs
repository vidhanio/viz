//! Typed messages pushed from the background server runtime or blocking
//! platform tasks into [`super::server::update`].

use viz_client::protocol::{
    ConfigOption, Entry, EntryData, EntryId, EntryPage, Environment, FileContent, FileSearchMatch,
    Forked, GitHubItem, Harness, Lifecycle, LiveDelta, Project, RequestId, SearchHit, Task,
    ThreadId, ThreadState, ThreadSummary, WorkspaceChange,
};

use crate::{
    effects::LoadedAttachments,
    model::{AttachmentTarget, Checkouts, DraftAttachment, SessionId, Status},
};

/// Events pushed from the background server runtime or blocking platform tasks.
#[derive(Debug)]
pub enum ServerMsg {
    /// Git status or RPC reply routed to [`super::git`].
    Git(super::GitMsg),
    /// Configured display endpoint string.
    Endpoint(String),
    /// Connection status changed (connecting or error).
    StatusChanged(Status),
    /// Initial handshake and workspace bootstrap succeeded.
    Connected {
        /// Active session identifier for subsequent session-scoped events.
        session: SessionId,
        /// Server version string.
        version: String,
        /// Active environment id reported by `hello`.
        environment: String,
        /// Monotonic workspace revision of the initial snapshot.
        rev: u64,
        /// Available environments.
        environments: Vec<Environment>,
        /// Available harnesses in this environment.
        harnesses: Vec<Harness>,
        /// Initial project list sorted for display.
        projects: Vec<Project>,
        /// Initial thread list sorted for display.
        threads: Vec<ThreadSummary>,
        /// Initial delegated task list.
        tasks: Vec<Task>,
        /// Thread already watched during reconnect, if any.
        watched: Option<ThreadId>,
    },
    /// Reply to an agent approval or question request finished.
    RequestAnswered {
        /// Request id that was answered.
        request: RequestId,
        /// Completion result or error string.
        result: Result<(), String>,
    },
    /// Authoritative project, thread, and task snapshot arrived.
    WorkspaceSynced {
        /// Environment id that produced the snapshot.
        environment: String,
        /// Monotonic workspace revision of this snapshot.
        rev: u64,
        /// Projects sorted for display.
        projects: Vec<Project>,
        /// Threads sorted for display.
        threads: Vec<ThreadSummary>,
        /// Delegated tasks tracked in this environment.
        tasks: Vec<Task>,
    },
    /// Coalesced revisioned workspace entity delta arrived.
    WorkspaceDelta {
        /// Environment id that produced the delta.
        environment: String,
        /// Monotonic workspace revision after applying `changes`.
        rev: u64,
        /// Incremental entity mutations.
        changes: Vec<WorkspaceChange>,
    },
    /// Authoritative delegated task list snapshot arrived.
    TasksSynced {
        /// Environment id that produced the snapshot.
        environment: String,
        /// Tasks tracked in this environment.
        tasks: Vec<Task>,
    },
    /// Harness catalog refreshed after a server `harnesses.changed` push.
    HarnessesSynced(Vec<Harness>),
    /// Latest authoritative state of a watched thread.
    ThreadState {
        thread: ThreadId,
        state: Box<ThreadState>,
    },
    /// Incremental ACP live turn delta for a watched thread.
    ThreadLive {
        /// Watched thread id.
        thread: ThreadId,
        /// Delta to apply to the thread's live turn state.
        delta: Box<LiveDelta>,
    },
    /// Transcript reset or incremental entries for a watched thread.
    ThreadView {
        /// Watched thread id.
        thread: ThreadId,
        /// Entries in ascending id order.
        entries: Vec<Entry>,
        /// Whether this replaces the viewport or appends.
        reset: bool,
        /// Exclusive cursor for loading older entries.
        before: Option<EntryId>,
    },
    /// Older transcript page finished loading (or failed).
    EarlierLoaded {
        /// Thread id.
        thread: ThreadId,
        /// Cursor that was requested.
        before: EntryId,
        /// Older entries, or `None` on failure.
        page: Option<EntryPage>,
    },
    /// Watching a newly selected thread failed; restore the prior watched
    /// thread.
    SelectFailed {
        /// Thread that failed to watch.
        failed: ThreadId,
        /// Previously watched thread, if any.
        previous: Option<ThreadId>,
        /// Error detail for toast.
        error: String,
    },
    /// Fresh harness catalog for a local draft finished loading.
    DraftHarnessesLoaded {
        /// Local draft sequence id.
        draft: u64,
        /// `None` when offline; `Some(result)` when queried from the server.
        result: Option<Result<Vec<Harness>, String>>,
    },
    /// Model-specific options finished loading for `harness`.
    HarnessOptionsLoaded {
        /// Environment id.
        environment: String,
        /// Harness id.
        harness: String,
        /// Model whose options were requested.
        model: String,
        /// Fresh config options or error string.
        result: Result<Vec<ConfigOption>, String>,
    },
    /// First send on a local draft created a real server thread.
    ThreadCreated {
        /// Local draft sequence id.
        draft: u64,
        /// New server thread id.
        thread: ThreadId,
    },
    /// Launching a local draft failed; restore prompt and attachments.
    LaunchFailed {
        /// Local draft sequence id.
        draft: u64,
        /// Submitted editor state.
        recovery: Box<crate::model::DraftData>,
        /// Toast title.
        title: String,
        /// Optional toast detail.
        detail: Option<String>,
    },
    /// Sending input to an existing thread failed; restore prompt and
    /// attachments.
    SendFailed {
        /// Destination thread.
        thread: ThreadId,
        /// Original editor snapshot, when submitted from the composer.
        recovery: Option<Box<crate::effects::DraftRecovery>>,
        /// Toast title.
        title: String,
        /// Optional toast detail.
        detail: Option<String>,
    },
    /// Server registered a new project.
    ProjectPalette {
        environment: String,
        request: u64,
        result: Box<Result<crate::model::projects::ProjectReply, String>>,
    },
    /// Git branches and worktrees finished loading for `project`.
    CheckoutsLoaded {
        /// Environment id.
        environment: String,
        /// Project path.
        project: String,
        /// Loaded checkouts or error string.
        result: Result<Checkouts, String>,
    },
    /// `@file` completion search finished.
    FileSearchResult {
        /// Monotonic completion request id.
        request: u64,
        /// Matching ranked file results or error string.
        result: Result<Vec<FileSearchMatch>, String>,
    },
    /// `#` GitHub pull-request and issue search finished.
    GitHubSearchResult {
        /// Monotonic completion request id.
        request: u64,
        /// Matching GitHub items or error string.
        result: Result<Vec<GitHubItem>, String>,
    },
    /// Palette full-text transcript search finished.
    ThreadSearchResult {
        /// Monotonic palette request id.
        request: u64,
        /// Matching hits or error string.
        result: Result<Vec<SearchHit>, String>,
    },
    /// Blocking attachment loader finished reading files or clipboard image.
    AttachmentsLoaded {
        /// Environment epoch when the load started.
        scope: u64,
        /// Composer target receiving the attachments.
        target: AttachmentTarget,
        /// Encoded attachments and any per-file errors.
        loaded: LoadedAttachments,
    },
    /// Optimistic mutation succeeded on the server.
    MutationOk {
        /// Optimistic mutation id.
        op_id: u64,
    },
    /// Thread triage mutation succeeded with authoritative server lifecycle.
    ThreadTriaged {
        /// Optimistic mutation id.
        op_id: u64,
        /// Target thread id.
        thread: ThreadId,
        /// Authoritative post-triage lifecycle.
        lifecycle: Box<Lifecycle>,
    },
    /// Server regenerated a thread's title.
    ThreadRetitled {
        /// Target thread id.
        thread: ThreadId,
        /// Regenerated title.
        title: String,
    },
    /// Server returned project icon bytes or confirmed no image exists.
    ProjectIconLoaded {
        /// Environment id.
        environment: String,
        /// Project path id.
        project: String,
        /// Updated `ETag` when an image exists.
        etag: Option<String>,
        /// `(media_type, raw_bytes)` when image data arrived.
        image: Option<(String, Vec<u8>)>,
    },
    /// Optimistic mutation failed on the server; roll back local cache and
    /// toast.
    MutationFailed {
        /// Optimistic mutation id.
        op_id: u64,
        /// Toast title.
        title: String,
        /// Error detail.
        detail: String,
    },
    /// Unclassified server request failure to surface via toast.
    RequestFailed {
        /// Toast title.
        title: String,
        /// Error detail.
        detail: String,
    },
    /// `thread.fork` created a branched thread.
    ThreadForked {
        /// Newly forked thread and optional excluded prompt.
        forked: Box<Forked>,
        /// Load the excluded prompt into the composer for editing.
        load_prompt: bool,
    },
    /// Content-plane full entry download finished for a trimmed item.
    EntryLoaded {
        /// Thread id.
        thread: ThreadId,
        /// Stable item key.
        key: String,
        /// Stored entry id.
        entry: EntryId,
        /// Decoded full payload or error string.
        result: Result<Box<EntryData>, String>,
    },
    /// `files.read` (and image content fetch when applicable) finished for the
    /// file preview modal.
    FilePreviewLoaded {
        /// Monotonic preview request serial.
        request: u64,
        /// Loaded file metadata/body plus optional preview image bytes, or an
        /// error string.
        result: Result<(FileContent, Option<Vec<u8>>), String>,
    },
    /// Content-channel attachment download finished for a stored thread
    /// attachment.
    ThreadAttachmentLoaded {
        /// Owning thread id.
        thread: ThreadId,
        /// Attachment SHA-256 id.
        id: String,
        /// Optional preview modal request serial.
        preview_request: Option<u64>,
        /// Loaded raw attachment or error string.
        result: Result<DraftAttachment, String>,
    },
}
