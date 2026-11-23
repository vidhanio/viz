//! Pure transitions for server responses, push notifications, and optimistic
//! rollbacks.

use std::sync::Arc;

use viz_client::protocol::{
    Attachment, ConfigOption, Entry, EntryId, Environment, Harness, LaunchThread, LiveDelta,
    Project, SearchHit, SendInput, Task, ThreadId, ThreadState, ThreadStatus, ThreadSummary,
};

use super::selection::{clear_selection, focus_draft, select_thread, show_thread};
use crate::{
    effects::{ATTACHMENT_ENCODED_LIMIT, EditSync, Effect, LoadedAttachments, ServerEffect},
    model::{
        AttachmentTarget, Checkouts, Model, OptimisticUndo, PaletteSearch, PaneSwitch, Status,
        Transcript, thread_title,
    },
};

/// Events pushed from the background server runtime or blocking platform tasks.
#[derive(Debug)]
pub enum ServerMsg {
    /// Configured display endpoint string.
    Endpoint(String),
    /// Connection status changed (connecting or error).
    StatusChanged(Status),
    /// Initial handshake and workspace bootstrap succeeded.
    Connected {
        /// Server version string.
        version: String,
        /// Active environment id reported by `hello`.
        environment: String,
        /// Available environments.
        environments: Vec<Environment>,
        /// Available harnesses in this environment.
        harnesses: Vec<Harness>,
        /// Thread already watched during reconnect, if any.
        watched: Option<ThreadId>,
    },
    /// Authoritative project and thread list snapshot arrived.
    WorkspaceSynced {
        /// Environment id that produced the snapshot.
        environment: String,
        /// Projects sorted for display.
        projects: Vec<Project>,
        /// Threads sorted for display.
        threads: Vec<ThreadSummary>,
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
    /// Live state update for a watched thread.
    ThreadState {
        /// Watched thread id.
        thread: ThreadId,
        /// Latest thread state.
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
        entries: Arc<Vec<Entry>>,
        /// Whether this replaces the viewport or appends.
        reset: bool,
        /// Exclusive cursor for loading older entries.
        before: Option<EntryId>,
    },
    /// Older transcript page finished loading (or failed).
    EarlierLoaded {
        /// Thread id.
        thread: ThreadId,
        /// Rebuilt transcript including older entries, or `None` on failure.
        transcript: Option<Transcript>,
    },
    /// Watching a newly selected thread failed; restore the prior watched
    /// thread.
    SelectFailed {
        /// Thread that failed to watch.
        failed: ThreadId,
        /// Previously watched thread, if any.
        previous: Option<ThreadId>,
        /// Cached entries of the previous thread.
        entries: Vec<Entry>,
        /// Cached pagination cursor of the previous thread.
        before: Option<EntryId>,
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
        /// Original launch params carrying prompt and attachments.
        params: Box<LaunchThread>,
        /// Toast title.
        title: String,
        /// Optional toast detail.
        detail: Option<String>,
    },
    /// Sending input to an existing thread failed; restore prompt and
    /// attachments.
    SendFailed {
        /// Original input params.
        input: Box<SendInput>,
        /// Toast title.
        title: String,
        /// Optional toast detail.
        detail: Option<String>,
    },
    /// Server registered a new project.
    ProjectAdded(Box<Project>),
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
        /// Thread id searched.
        thread: ThreadId,
        /// Monotonic completion request id.
        request: u64,
        /// Matching paths or error string.
        result: Result<Vec<String>, String>,
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
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive dispatch over every typed server response and notification"
)]
pub(super) fn update(model: &mut Model, msg: ServerMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        ServerMsg::Endpoint(endpoint) => {
            model.connection.endpoint = Some(endpoint);
        }
        ServerMsg::StatusChanged(status) => {
            model.connection.status = status;
            effects.push(Effect::SetWindowTitle(model.window_title()));
        }
        ServerMsg::Connected {
            version,
            environment,
            environments,
            harnesses,
            watched,
        } => {
            model.connection.status = Status::Connected { version };
            model.connection.environment = Some(environment);
            model.connection.environments = environments;
            model.cache_mut().harnesses = harnesses;
            if model.selection.new_thread.is_none() {
                if let Some(thread) = watched {
                    show_thread(model, thread, &mut effects);
                    effects.push(focus_draft());
                } else {
                    clear_selection(model, &mut effects);
                }
            }
            effects.push(Effect::SetWindowTitle(model.window_title()));
        }
        ServerMsg::WorkspaceSynced {
            environment,
            projects,
            threads,
        } => {
            if model
                .connection
                .environment
                .as_deref()
                .is_none_or(|active| active == environment)
            {
                apply_workspace_sync(model, projects, threads, &mut effects);
            }
        }
        ServerMsg::TasksSynced { environment, tasks } => {
            if model
                .connection
                .environment
                .as_deref()
                .is_none_or(|active| active == environment)
            {
                model.cache_mut().tasks = tasks;
            }
        }
        ServerMsg::HarnessesSynced(harnesses) => {
            model.cache_mut().harnesses = harnesses;
        }
        ServerMsg::ThreadState { thread, state } => {
            if model.selection.selected != Some(thread) {
                return effects;
            }
            let status_changed = model.thread_state().map(|prev| prev.status) != Some(state.status);
            let open = model.cache_mut().open_thread_mut(thread);
            open.transcript.sync_acp(state.live.as_ref());
            open.state = Some(*state);
            model.composer.model_menu.loading_levels = None;
            if status_changed {
                effects.push(Effect::SetWindowTitle(model.window_title()));
            }
        }
        ServerMsg::ThreadLive { thread, delta } => {
            if model.selection.selected == Some(thread) {
                let open = model.cache_mut().open_thread_mut(thread);
                if let Some(state) = &mut open.state {
                    delta.apply(&mut state.live);
                    open.transcript.sync_acp(state.live.as_ref());
                }
            }
        }
        ServerMsg::ThreadView {
            thread,
            entries,
            reset,
            before,
        } => {
            if model.selection.selected != Some(thread) {
                return effects;
            }
            let open = model.cache_mut().open_thread_mut(thread);
            if reset {
                open.transcript = Transcript::from_entries(entries.iter(), before);
                open.loading = false;
                open.history_loading = false;
            } else {
                open.transcript.append(entries.iter());
            }
            let live = open.state.as_ref().and_then(|state| state.live.as_ref());
            open.transcript.sync_acp(live);
        }
        ServerMsg::EarlierLoaded { thread, transcript } => {
            if model.selection.selected != Some(thread) {
                return effects;
            }
            let first_key = model
                .transcript()
                .items
                .first()
                .map(|item| item.key().to_owned());
            let open = model.cache_mut().open_thread_mut(thread);
            open.history_loading = false;
            if let Some(transcript) = transcript {
                if let Some(first_key) = first_key {
                    effects.push(Effect::KeepOnScreen(first_key));
                }
                open.transcript = transcript;
                let live = open.state.as_ref().and_then(|state| state.live.as_ref());
                open.transcript.sync_acp(live);
            }
        }
        ServerMsg::SelectFailed {
            failed,
            previous,
            entries,
            before,
            error,
        } => {
            if model.selection.selected == Some(failed) {
                if let Some(previous) = previous {
                    show_thread(model, previous, &mut effects);
                    let open = model.cache_mut().open_thread_mut(previous);
                    open.transcript = Transcript::from_entries(&entries, before);
                    open.loading = false;
                    open.history_loading = false;
                    let live = open.state.as_ref().and_then(|state| state.live.as_ref());
                    open.transcript.sync_acp(live);
                } else {
                    clear_selection(model, &mut effects);
                }
                effects.push(Effect::SetWindowTitle(model.window_title()));
            }
            effects.push(Effect::Toast {
                title: "Request failed".into(),
                detail: Some(error),
            });
        }
        ServerMsg::DraftHarnessesLoaded { draft, result } => {
            if model
                .selection
                .new_thread
                .as_ref()
                .is_none_or(|thread| thread.id != draft)
            {
                return effects;
            }
            match result {
                Some(Ok(harnesses)) => model.cache_mut().harnesses = harnesses,
                Some(Err(error)) => effects.push(Effect::Toast {
                    title: "Couldn’t load harnesses".into(),
                    detail: Some(error),
                }),
                None => {}
            }
            if let Some(thread) = model
                .selection
                .new_thread
                .as_mut()
                .filter(|thread| thread.id == draft)
            {
                thread.loading_choices = false;
            }
        }
        ServerMsg::HarnessOptionsLoaded {
            environment,
            harness,
            result,
        } => {
            if model.connection.environment.as_deref() != Some(environment.as_str()) {
                return effects;
            }
            match result {
                Ok(options) => {
                    if let Some(runner) = model
                        .cache_mut()
                        .harnesses
                        .iter_mut()
                        .find(|runner| runner.id == harness)
                    {
                        runner.options = options;
                    }
                    model.composer.model_menu.loading_levels = None;
                }
                Err(error) => {
                    model.composer.model_menu.loading_levels = None;
                    effects.push(Effect::Toast {
                        title: "Couldn’t load model options".into(),
                        detail: Some(error),
                    });
                }
            }
        }
        ServerMsg::ThreadCreated { draft, thread } => {
            let Some(project) = model
                .selection
                .new_thread
                .as_ref()
                .filter(|pending| pending.id == draft)
                .map(|pending| pending.project.clone())
            else {
                return effects;
            };
            let expanded = project
                .as_deref()
                .is_some_and(|path| model.sidebar.collapsed_projects.remove(path));
            model.selection.new_thread = None;
            model.selection.selected = Some(thread);
            model.selection.switch = PaneSwitch::Cut;
            let open = model.cache_mut().open_thread_mut(thread);
            open.transcript = Transcript::default();
            open.state = None;
            open.loading = true;
            open.history_loading = false;
            effects.push(focus_draft());
            effects.push(Effect::Server(ServerEffect::Select(thread)));
            if expanded {
                effects.push(Effect::SavePrefs(model.to_prefs()));
            }
        }
        ServerMsg::LaunchFailed {
            draft,
            params,
            title,
            detail,
        } => {
            if let Some(thread) = model
                .selection
                .new_thread
                .as_mut()
                .filter(|thread| thread.id == draft)
            {
                thread.sending = false;
                let text = params.text.unwrap_or_default();
                prepend_draft_data(
                    &mut model.composer.draft.text,
                    &mut model.composer.draft.attachments,
                    &text,
                    params.attachments,
                );
                if !text.is_empty() {
                    effects.push(Effect::SyncEdit(EditSync::PrependDraft(text)));
                }
            }
            effects.push(Effect::Toast { title, detail });
        }
        ServerMsg::SendFailed {
            input,
            title,
            detail,
        } => {
            let thread = input.thread;
            let text = input.text;
            let files = input.attachments;
            if model.selection.selected == Some(thread) {
                prepend_draft_data(
                    &mut model.composer.draft.text,
                    &mut model.composer.draft.attachments,
                    &text,
                    files,
                );
                if !text.is_empty() {
                    effects.push(Effect::SyncEdit(EditSync::PrependDraft(text)));
                }
            } else {
                let env = model.active_env_key().to_owned();
                let entry = model
                    .composer
                    .drafts_by_env
                    .entry(env)
                    .or_default()
                    .entry(thread)
                    .or_default();
                prepend_draft_data(&mut entry.text, &mut entry.attachments, &text, files);
            }
            effects.push(Effect::Toast { title, detail });
        }
        ServerMsg::ProjectAdded(project) => {
            if let Some(thread) = &mut model.selection.new_thread
                && !thread.sending
            {
                thread.set_project(Some(&project));
            }
            model.cache_mut().insert_project_sorted(*project);
        }
        ServerMsg::CheckoutsLoaded {
            environment,
            project,
            result,
        } => {
            if model.connection.environment.as_deref() != Some(environment.as_str()) {
                return effects;
            }
            match result {
                Ok(checkouts) => {
                    model.cache_mut().checkouts.insert(project, checkouts);
                }
                Err(error) => effects.push(Effect::Toast {
                    title: "Couldn’t list branches".into(),
                    detail: Some(error),
                }),
            }
        }
        ServerMsg::FileSearchResult {
            thread,
            request,
            result,
        } => {
            if model.selection.selected == Some(thread)
                && model.composer.completion.request == request
            {
                model.composer.completion.files = Some(result);
            }
        }
        ServerMsg::ThreadSearchResult { request, result } => {
            if model.palette.open
                && model.palette.request == request
                && matches!(model.palette.search, PaletteSearch::Pending)
            {
                model.palette.search = PaletteSearch::Complete(result);
            }
        }
        ServerMsg::AttachmentsLoaded {
            scope,
            target,
            loaded,
        } => apply_attachments_loaded(model, scope, target, loaded, &mut effects),
        ServerMsg::MutationOk { op_id } => {
            model.optimistic.commit(op_id);
        }
        ServerMsg::MutationFailed {
            op_id,
            title,
            detail,
        } => {
            if let Some(undo) = model.optimistic.take(op_id) {
                rollback_optimistic(model, undo);
            }
            effects.push(Effect::Toast {
                title,
                detail: Some(detail),
            });
        }
        ServerMsg::RequestFailed { title, detail } => {
            effects.push(Effect::Toast {
                title,
                detail: Some(detail),
            });
        }
    }
    effects
}

fn apply_workspace_sync(
    model: &mut Model,
    projects: Vec<Project>,
    threads: Vec<ThreadSummary>,
    effects: &mut Vec<Effect>,
) {
    use ThreadStatus::{Failed, Idle, Working};

    for thread in &threads {
        let Some(before) = model.summary(thread.id).map(|old| old.status) else {
            continue;
        };
        let ran = before == Working;
        let watched = model.disclosure.focused && model.selection.selected == Some(thread.id);
        let body = match thread.status {
            _ if before == thread.status || watched => continue,
            _ if thread.parent.is_some() => continue,
            Idle if ran => "Finished",
            Failed if ran => "Failed",
            _ => continue,
        };
        effects.push(Effect::Notify {
            title: thread_title(thread).to_owned(),
            body,
        });
    }
    let fallback = model
        .selection
        .selected
        .and_then(|id| model.summary(id))
        .filter(|thread| !threads.iter().any(|current| current.id == thread.id))
        .and_then(|old| {
            let mut ancestor = old.parent.filter(|_| {
                old.parent_environment
                    .as_ref()
                    .is_none_or(|env| Some(env) == model.connection.environment.as_ref())
            });
            while let Some(id) = ancestor {
                if threads.iter().any(|current| current.id == id) {
                    return Some(id);
                }
                ancestor = model.summary(id).and_then(|parent| parent.parent);
            }
            None
        });
    let cache = model.cache_mut();
    cache.set_projects(projects);
    cache.set_threads(threads);
    if model.connected()
        && model.selection.new_thread.is_none()
        && model
            .selection
            .selected
            .is_some_and(|id| model.summary(id).is_none())
    {
        if let Some(id) = fallback {
            select_thread(model, id, effects);
        } else {
            clear_selection(model, effects);
            effects.push(Effect::Server(ServerEffect::ClearSelection));
        }
    }
}

fn apply_attachments_loaded(
    model: &mut Model,
    scope: u64,
    target: AttachmentTarget,
    loaded: LoadedAttachments,
    effects: &mut Vec<Effect>,
) {
    if scope != model.connection.epoch || !model.composer.attachment_reads.remove(&target) {
        return;
    }
    let mut errors = loaded.errors;
    if !loaded.attachments.is_empty() {
        let env = model.active_env_key().to_owned();
        let existing = match target {
            AttachmentTarget::Draft(id) => {
                if model
                    .selection
                    .new_thread
                    .as_ref()
                    .is_none_or(|thread| thread.id != id)
                {
                    return;
                }
                &mut model.composer.draft.attachments
            }
            AttachmentTarget::Thread(id) if model.selection.selected == Some(id) => {
                &mut model.composer.draft.attachments
            }
            AttachmentTarget::Thread(id) => {
                &mut model
                    .composer
                    .drafts_by_env
                    .entry(env)
                    .or_default()
                    .entry(id)
                    .or_default()
                    .attachments
            }
        };
        let mut size: usize = existing.iter().map(|file| file.data.len()).sum();
        for file in loaded.attachments {
            if size + file.data.len() > ATTACHMENT_ENCODED_LIMIT {
                errors.push(format!(
                    "{}: attachments exceed the message limit",
                    file.name
                ));
            } else {
                size += file.data.len();
                existing.push(file);
            }
        }
    }
    if !errors.is_empty() {
        effects.push(Effect::Toast {
            title: "Couldn’t attach files".into(),
            detail: Some(errors.join("\n")),
        });
    }
}

fn prepend_draft_data(
    draft_text: &mut String,
    draft_attachments: &mut Vec<Attachment>,
    text: &str,
    mut files: Vec<Attachment>,
) {
    if !text.is_empty() {
        if draft_text.is_empty() {
            draft_text.push_str(text);
        } else {
            let mut combined = String::with_capacity(text.len() + 2 + draft_text.len());
            combined.push_str(text);
            combined.push_str("\n\n");
            combined.push_str(draft_text);
            *draft_text = combined;
        }
    }
    files.append(draft_attachments);
    *draft_attachments = files;
}

fn rollback_optimistic(model: &mut Model, undo: OptimisticUndo) {
    match undo {
        OptimisticUndo::ThreadSummary { env, id, prior } => {
            if let Some(cache) = model.caches.get_mut(&env)
                && let Some(current) = cache.summary_mut(id)
            {
                *current = *prior;
            }
        }
        OptimisticUndo::ProjectRename {
            env,
            path,
            prior_name,
        } => {
            if let Some(cache) = model.caches.get_mut(&env)
                && let Some(project) = cache.project_mut(&path)
            {
                project.name = prior_name;
            }
        }
        OptimisticUndo::ProjectRemove {
            env,
            index,
            project,
        } => {
            if let Some(cache) = model.caches.get_mut(&env) {
                cache.restore_project(index, *project);
            }
        }
    }
}
