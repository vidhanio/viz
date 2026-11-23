//! Pure helper transitions for workspace sync, thread view updates, failed
//! send recovery, attachment ingestion, and optimistic rollback.

use std::sync::Arc;

use viz_client::protocol::{
    Attachment, Entry, EntryId, LaunchThread, Lifecycle, Project, ProjectIcon, SendInput, ThreadId,
    ThreadState, ThreadStatus, ThreadSummary,
};

use super::selection::{clear_selection, focus_draft, select_thread, show_thread};
use crate::{
    effects::{ATTACHMENT_ENCODED_LIMIT, EditSync, Effect, LoadedAttachments, ServerEffect},
    model::{
        AttachmentTarget, Model, OptimisticUndo, PaneSwitch, Transcript, sync_summary_derived,
        thread_title,
    },
};

pub(super) fn is_active_env(model: &Model, env: &str) -> bool {
    model
        .connection
        .environment
        .as_deref()
        .is_none_or(|active| active == env)
}

pub(super) fn apply_workspace_sync(
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
    let icon_targets: Vec<Project> = model.cache().projects().cloned().collect();
    for project in &icon_targets {
        queue_project_icon(model, project, effects);
    }
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

pub(super) fn apply_thread_state(
    model: &mut Model,
    thread: ThreadId,
    state: ThreadState,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected != Some(thread) {
        return;
    }
    let status_changed = model.thread_state().map(|prev| prev.status) != Some(state.status);
    let open = model.cache_mut().open_thread_mut(thread);
    open.transcript.sync_acp(state.live.as_ref());
    open.state = Some(state);
    model.composer.model_menu.loading_levels = None;
    if status_changed {
        effects.push(Effect::SetWindowTitle(model.window_title()));
    }
}

pub(super) fn apply_thread_view(
    model: &mut Model,
    thread: ThreadId,
    entries: &Arc<Vec<Entry>>,
    reset: bool,
    before: Option<EntryId>,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected != Some(thread) {
        return;
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
    super::palette::continue_message_jump(model, effects);
}

pub(super) fn apply_earlier_loaded(
    model: &mut Model,
    thread: ThreadId,
    transcript: Option<Transcript>,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected != Some(thread) {
        return;
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
        super::palette::continue_message_jump(model, effects);
    } else if model
        .palette
        .message_jump
        .is_some_and(|(target, _)| target == thread)
    {
        model.palette.message_jump = None;
    }
}

pub(super) fn apply_select_failed(
    model: &mut Model,
    failed: ThreadId,
    previous: Option<ThreadId>,
    entries: &[Entry],
    before: Option<EntryId>,
    error: String,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected == Some(failed) {
        if let Some(previous) = previous {
            show_thread(model, previous, effects);
            let open = model.cache_mut().open_thread_mut(previous);
            open.transcript = Transcript::from_entries(entries, before);
            open.loading = false;
            open.history_loading = false;
            let live = open.state.as_ref().and_then(|state| state.live.as_ref());
            open.transcript.sync_acp(live);
        } else {
            clear_selection(model, effects);
        }
        effects.push(Effect::SetWindowTitle(model.window_title()));
    }
    effects.push(Effect::Toast {
        title: "Request failed".into(),
        detail: Some(error),
    });
}

pub(super) fn apply_thread_created(
    model: &mut Model,
    draft: u64,
    thread: ThreadId,
    effects: &mut Vec<Effect>,
) {
    if model
        .selection
        .new_thread
        .as_ref()
        .is_none_or(|pending| pending.id != draft)
    {
        return;
    }
    let access = model
        .selection
        .new_thread
        .take()
        .and_then(|draft| draft.access);
    if access.is_some() {
        effects.push(Effect::Server(ServerEffect::Access { thread, access }));
    }
    model.selection.selected = Some(thread);
    model.selection.switch = PaneSwitch::Cut;
    let open = model.cache_mut().open_thread_mut(thread);
    open.transcript = Transcript::default();
    open.state = None;
    open.loading = true;
    open.history_loading = false;
    effects.push(focus_draft());
    effects.push(Effect::Server(ServerEffect::Select(thread)));
}

pub(super) fn apply_thread_triaged(
    model: &mut Model,
    op_id: u64,
    thread: ThreadId,
    lifecycle: Lifecycle,
) {
    model.optimistic.commit(op_id);
    let now = model.disclosure.tool_clock;
    if let Some(summary) = model.cache_mut().summary_mut(thread) {
        summary.lifecycle = lifecycle;
        sync_summary_derived(summary, now);
    }
}

pub(super) fn queue_project_icon(model: &mut Model, project: &Project, effects: &mut Vec<Effect>) {
    let needs_fetch = matches!(
        project.settings.icon,
        ProjectIcon::Automatic | ProjectIcon::OwnerAvatar | ProjectIcon::Uploaded { .. }
    );
    if !needs_fetch {
        return;
    }
    let entry = model
        .cache_mut()
        .project_icons
        .entry(project.path.clone())
        .or_default();
    if entry.requested {
        return;
    }
    entry.requested = true;
    let etag = entry.etag.clone();
    effects.push(Effect::Server(ServerEffect::ReadProjectIcon {
        project: project.path.clone(),
        etag,
    }));
}

pub(super) fn apply_project_icon_loaded(
    model: &mut Model,
    environment: &str,
    project: &str,
    etag: Option<String>,
    image: Option<(String, Vec<u8>)>,
    effects: &mut Vec<Effect>,
) {
    if model.active_env_key() != environment {
        return;
    }
    let entry = model
        .cache_mut()
        .project_icons
        .entry(project.to_owned())
        .or_default();
    entry.etag = etag;
    entry.has_image = image.is_some();
    if let Some((media_type, bytes)) = image {
        effects.push(Effect::DecodeProjectIcon {
            environment: environment.to_owned(),
            project: project.to_owned(),
            media_type,
            bytes,
        });
    }
}

pub(super) fn apply_launch_failed(
    model: &mut Model,
    draft: u64,
    params: LaunchThread,
    title: String,
    detail: Option<String>,
    effects: &mut Vec<Effect>,
) {
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

pub(super) fn apply_send_failed(
    model: &mut Model,
    input: SendInput,
    title: String,
    detail: Option<String>,
    effects: &mut Vec<Effect>,
) {
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

pub(super) fn apply_attachments_loaded(
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

pub(super) fn rollback_optimistic(model: &mut Model, undo: OptimisticUndo) {
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
