//! Pure transitions for server responses, push notifications, and optimistic
//! rollbacks.

use std::sync::Arc;

use viz_client::protocol::{
    ConfigOption, Entry, EntryId, Environment, Harness, LaunchThread, Lifecycle, LiveDelta,
    Project, SearchHit, SendInput, Task, ThreadId, ThreadState, ThreadSummary,
};

use super::{
    selection::{clear_selection, focus_draft, select_thread, show_thread},
    server_sync::{
        apply_attachments_loaded, apply_earlier_loaded, apply_launch_failed,
        apply_project_icon_loaded, apply_select_failed, apply_send_failed, apply_thread_created,
        apply_thread_state, apply_thread_triaged, apply_thread_view, apply_workspace_sync,
        is_active_env, queue_project_icon, rollback_optimistic,
    },
};
use crate::{
    effects::{Effect, LoadedAttachments},
    model::{AttachmentTarget, Checkouts, Model, PaletteSearch, Status, Transcript},
};

/// Events pushed from the background server runtime or blocking platform tasks.
#[derive(Debug)]
pub enum ServerMsg {
    Git(super::GitMsg),
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
    /// Server created a fork of a thread.
    ThreadForked {
        /// Newly created forked thread id.
        thread: ThreadId,
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
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive dispatch over every typed server response and notification"
)]
pub(super) fn update(model: &mut Model, msg: ServerMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        ServerMsg::Git(msg) => return super::git::update(model, msg),
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
        } if is_active_env(model, &environment) => {
            model.connection.environment.get_or_insert(environment);
            apply_workspace_sync(model, projects, threads, &mut effects);
        }
        ServerMsg::TasksSynced { environment, tasks } if is_active_env(model, &environment) => {
            model.connection.environment.get_or_insert(environment);
            model.cache_mut().tasks = tasks;
        }
        ServerMsg::WorkspaceSynced { .. } | ServerMsg::TasksSynced { .. } => {}
        ServerMsg::HarnessesSynced(harnesses) => {
            model.cache_mut().harnesses = harnesses;
        }
        ServerMsg::ThreadState { thread, state } => {
            apply_thread_state(model, thread, *state, &mut effects);
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
        } => apply_thread_view(model, thread, &entries, reset, before),
        ServerMsg::EarlierLoaded { thread, transcript } => {
            apply_earlier_loaded(model, thread, transcript, &mut effects);
        }
        ServerMsg::SelectFailed {
            failed,
            previous,
            entries,
            before,
            error,
        } => apply_select_failed(
            model,
            failed,
            previous,
            &entries,
            before,
            error,
            &mut effects,
        ),
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
            apply_thread_created(model, draft, thread, &mut effects);
        }
        ServerMsg::LaunchFailed {
            draft,
            params,
            title,
            detail,
        } => apply_launch_failed(model, draft, *params, title, detail, &mut effects),
        ServerMsg::SendFailed {
            input,
            title,
            detail,
        } => apply_send_failed(model, *input, title, detail, &mut effects),
        ServerMsg::ProjectAdded(project) => {
            if let Some(thread) = &mut model.selection.new_thread
                && !thread.sending
            {
                thread.set_project(Some(&project));
            }
            let icon_target = (*project).clone();
            model.cache_mut().insert_project_sorted(*project);
            queue_project_icon(model, &icon_target, &mut effects);
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
        ServerMsg::MutationOk { op_id } => model.optimistic.commit(op_id),
        ServerMsg::ThreadTriaged {
            op_id,
            thread,
            lifecycle,
        } => {
            apply_thread_triaged(model, op_id, thread, *lifecycle);
        }
        ServerMsg::ThreadRetitled { thread, title } => {
            if let Some(summary) = model.cache_mut().summary_mut(thread) {
                summary.title = title;
            }
        }
        ServerMsg::ThreadForked { thread } => select_thread(model, thread, &mut effects),
        ServerMsg::ProjectIconLoaded {
            environment,
            project,
            etag,
            image,
        } => {
            apply_project_icon_loaded(model, &environment, &project, etag, image, &mut effects);
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
