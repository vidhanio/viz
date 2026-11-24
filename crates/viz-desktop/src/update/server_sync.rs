//! Pure helper transitions for workspace sync, thread view updates, failed
//! send recovery, attachment ingestion, and optimistic rollback.

use viz_client::protocol::{
    Entry, EntryAnchor, EntryData, EntryId, EntryPage, Lifecycle, Project, ProjectIcon, Task,
    ThreadId, ThreadState, ThreadStatus, ThreadSummary, WorkspaceChange,
};

use super::selection::{clear_selection, focus_draft, select_thread, show_thread};
use crate::{
    effects::{ATTACHMENT_TOTAL_BYTES, EditSync, Effect, LoadedAttachments, ServerEffect},
    model::{
        AttachmentStatus, AttachmentTarget, DraftData, Model, OptimisticUndo, PaneSwitch,
        Transcript, sync_summary_derived, thread_title,
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
    rev: u64,
    projects: Vec<Project>,
    threads: Vec<ThreadSummary>,
    tasks: Vec<Task>,
    effects: &mut Vec<Effect>,
) {
    for thread in &threads {
        maybe_notify_thread_finished(model, thread, effects);
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
    cache.rev = cache.rev.max(rev);
    cache.set_projects(projects);
    cache.set_threads(threads);
    cache.tasks = tasks;
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

/// Applies incremental revisioned entity changes without full workspace list
/// reloads, retiring superseded optimistic edits against `rev`.
pub(super) fn apply_workspace_delta(
    model: &mut Model,
    rev: u64,
    changes: Vec<WorkspaceChange>,
    effects: &mut Vec<Effect>,
) {
    let env = model.active_env_key().to_owned();
    let mut selected_removed = false;
    let mut fallback = None;
    for change in changes {
        match change {
            WorkspaceChange::ThreadUpsert { thread } => {
                let thread = *thread;
                let id = thread.id;
                maybe_notify_thread_finished(model, &thread, effects);
                model.cache_mut().upsert_thread(thread);
                model.optimistic.retire_thread(&env, id, rev);
            }
            WorkspaceChange::ThreadRemoved { thread } => {
                if model.selection.selected == Some(thread) {
                    selected_removed = true;
                    fallback = ancestor_fallback(model, thread);
                }
                model.cache_mut().remove_thread(thread);
                model.optimistic.retire_thread(&env, thread, rev);
            }
            WorkspaceChange::ProjectUpsert { project } => {
                let project = *project;
                let path = project.path.clone();
                let icon_changed = model
                    .cache()
                    .project(&path)
                    .is_some_and(|old| old.settings.icon != project.settings.icon);
                model.cache_mut().upsert_project(project.clone());
                if icon_changed && let Some(icon) = model.cache_mut().project_icons.get_mut(&path) {
                    icon.requested = false;
                }
                queue_project_icon(model, &project, effects);
                model.optimistic.retire_project(&env, &path, rev);
            }
            WorkspaceChange::ProjectRemoved { project } => {
                model.cache_mut().remove_project(&project);
                model.cache_mut().project_icons.remove(&project);
                model.optimistic.retire_project(&env, &project, rev);
            }
            WorkspaceChange::TaskUpsert { task } => {
                model.cache_mut().upsert_task(*task);
            }
            WorkspaceChange::TaskRemoved { task } => {
                model.cache_mut().remove_task(&task.0);
            }
            WorkspaceChange::Resync => {}
        }
    }
    let cache = model.cache_mut();
    cache.rev = cache.rev.max(rev);
    if selected_removed
        && model.connected()
        && model.selection.new_thread.is_none()
        && model
            .selection
            .selected
            .is_some_and(|id| model.summary(id).is_none())
    {
        if let Some(id) = fallback.filter(|id| model.summary(*id).is_some()) {
            select_thread(model, id, effects);
        } else {
            clear_selection(model, effects);
            effects.push(Effect::Server(ServerEffect::ClearSelection));
        }
    }
}

fn maybe_notify_thread_finished(model: &Model, thread: &ThreadSummary, effects: &mut Vec<Effect>) {
    use ThreadStatus::{Failed, Idle, Working};

    let Some(before) = model.summary(thread.id).map(|old| old.status) else {
        return;
    };
    let ran = before == Working;
    let watched = model.disclosure.focused && model.selection.selected == Some(thread.id);
    let body = match thread.status {
        _ if before == thread.status || watched => return,
        _ if thread.parent.is_some() => return,
        Idle if ran => "Finished",
        Failed if ran => "Failed",
        _ => return,
    };
    effects.push(Effect::Notify {
        thread: thread.id,
        title: thread_title(thread).to_owned(),
        body,
    });
}

fn ancestor_fallback(model: &Model, removed: ThreadId) -> Option<ThreadId> {
    let old = model.summary(removed)?;
    let mut ancestor = old.parent.filter(|_| {
        old.parent_environment
            .as_ref()
            .is_none_or(|env| Some(env) == model.connection.environment.as_ref())
    });
    while let Some(id) = ancestor {
        if id != removed && model.summary(id).is_some() {
            return Some(id);
        }
        ancestor = model.summary(id).and_then(|parent| parent.parent);
    }
    None
}

pub(super) fn apply_thread_state(
    model: &mut Model,
    thread: ThreadId,
    mut state: ThreadState,
    effects: &mut Vec<Effect>,
) {
    let live = state.live.take();
    let open = model.cache_mut().open_thread_mut(thread);
    let status_changed = open.state.as_ref().map(|prev| prev.status) != Some(state.status);
    open.transcript.sync_acp(live.as_ref());
    open.state = Some(state);
    if model.selection.selected == Some(thread) {
        model.composer.model_menu.loading_levels = None;
        if status_changed {
            effects.push(Effect::SetWindowTitle(model.window_title()));
        }
    }
}

pub(super) fn apply_thread_view(
    model: &mut Model,
    thread: ThreadId,
    entries: &[Entry],
    reset: bool,
    before: Option<EntryId>,
    effects: &mut Vec<Effect>,
) {
    if reset {
        model.cache_mut().retain_open_thread(thread);
    }
    let open = model.cache_mut().open_thread_mut(thread);
    if reset {
        let mut transcript = Transcript::from_entries(entries.iter(), before);
        transcript.rev = open.transcript.rev.wrapping_add(1);
        open.transcript = transcript;
        open.history = None;
        open.loading = false;
        open.history_request = None;
        open.media_slots.clear();
    } else {
        open.transcript.append(entries.iter());
    }
    if !entries.is_empty() {
        queue_entry_image_attachments(model, thread, entries, effects);
    }
    if model.selection.selected == Some(thread) {
        super::palette::continue_message_jump(model, effects);
    }
}

pub(super) fn apply_history_loaded(
    model: &mut Model,
    thread: ThreadId,
    anchor: EntryAnchor,
    page: Option<EntryPage>,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected != Some(thread) || model.history_request() != Some(anchor) {
        return;
    }
    let open = model.cache_mut().open_thread_mut(thread);
    open.history_request = None;
    let Some(page) = page else {
        effects.push(Effect::Toast {
            title: "Couldn’t load message history".into(),
            detail: None,
        });
        return;
    };
    match anchor {
        EntryAnchor::Before { .. } => {
            let transcript = open
                .history
                .as_mut()
                .map_or(&mut open.transcript, |window| &mut window.transcript);
            transcript.prepend(&page);
            if let Some(window) = &open.history
                && window.at_bottom == Some(true)
                && let Some(entry) = window.after
            {
                super::palette::request_history(
                    model,
                    thread,
                    EntryAnchor::After { entry },
                    effects,
                );
            }
        }
        EntryAnchor::Around { entry } => {
            let transcript = Transcript::from_entries(&page.entries, page.before);
            if let Some(target) = crate::timeline::message_row(&transcript, entry) {
                open.history = Some(crate::model::HistoryWindow {
                    transcript,
                    after: page.after,
                    at_bottom: None,
                });
                model.timeline.far_from_bottom = false;
                model.timeline.bump_fold_rev();
                super::palette::reveal_message(model, thread, target, effects);
            } else {
                effects.push(Effect::Toast {
                    title: "Message is no longer available".into(),
                    detail: None,
                });
            }
        }
        EntryAnchor::After { .. } => {
            if let Some(window) = &mut open.history {
                window.transcript.append(&page.entries);
                window.after = page.after;
                if window.after.is_none() {
                    open.history = None;
                    model.timeline.bump_fold_rev();
                    effects.push(Effect::ScrollToBottom(model.timeline_page_key()));
                }
            }
        }
        EntryAnchor::Latest => return,
    }
    queue_entry_image_attachments(model, thread, &page.entries, effects);
}

pub(super) fn apply_select_failed(
    model: &mut Model,
    failed: ThreadId,
    previous: Option<ThreadId>,
    error: String,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected == Some(failed) {
        if let Some(previous) = previous {
            show_thread(model, previous, effects);
            model.cache_mut().open_thread_mut(previous).history_request = None;
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
    model.selection.new_thread = None;
    model.selection.selected = Some(thread);
    model.selection.switch = PaneSwitch::Cut;
    let open = model.cache_mut().open_thread_mut(thread);
    open.transcript = Transcript::default();
    open.state = None;
    open.loading = true;
    open.history_request = None;
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
    recovery: DraftData,
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
        restore_active_draft(model, recovery, effects);
    }
    effects.push(Effect::Toast { title, detail });
}

pub(super) fn apply_send_failed(
    model: &mut Model,
    thread: ThreadId,
    recovery: Option<crate::effects::DraftRecovery>,
    title: String,
    detail: Option<String>,
    effects: &mut Vec<Effect>,
) {
    if let Some(recovery) = recovery {
        if model.active_env_key() == recovery.environment
            && model.selection.selected == Some(thread)
        {
            restore_active_draft(model, recovery.draft, effects);
        } else {
            let saved = model
                .envs
                .entry(recovery.environment)
                .or_default()
                .drafts
                .entry(thread)
                .or_default();
            prepend_draft(saved, recovery.draft);
        }
        effects.push(Effect::SavePrefs(model.to_prefs()));
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
    if scope != model.connection.epoch || !model.connection.live.attachment_reads.remove(&target) {
        return;
    }
    let mut errors = loaded.errors;
    if !loaded.attachments.is_empty() {
        let env = model.active_env_key().to_owned();
        let (is_active, draft) = match target {
            AttachmentTarget::Draft(id) => {
                if model
                    .selection
                    .new_thread
                    .as_ref()
                    .is_none_or(|thread| thread.id != id)
                {
                    return;
                }
                (true, &mut model.composer.draft)
            }
            AttachmentTarget::Thread(id) if model.selection.selected == Some(id) => {
                (true, &mut model.composer.draft)
            }
            AttachmentTarget::Thread(id) => (
                false,
                model
                    .envs
                    .entry(env)
                    .or_default()
                    .drafts
                    .entry(id)
                    .or_default(),
            ),
        };
        let existing = std::sync::Arc::make_mut(&mut draft.attachments);
        let prior_len = existing.len();
        let mut size: usize = existing.iter().map(|file| file.bytes.len()).sum();
        for file in loaded.attachments {
            if size + file.bytes.len() > ATTACHMENT_TOTAL_BYTES {
                errors.push(format!(
                    "{}: attachments exceed the message limit",
                    file.name
                ));
            } else {
                size += file.bytes.len();
                existing.push(file);
            }
        }
        if existing.len() > prior_len {
            let refs = existing[prior_len..]
                .iter()
                .map(crate::model::DraftAttachment::pill)
                .collect::<Vec<_>>();
            if is_active {
                effects.push(Effect::SyncEdit(EditSync::InsertDraftRefs(refs)));
            } else {
                crate::composer::codec::append(draft, refs);
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
            if let Some(cache) = model.envs.get_mut(&env)
                && let Some(current) = cache.summary_mut(id)
            {
                *current = *prior;
            }
        }
        OptimisticUndo::ProjectPin {
            env,
            path,
            prior_pinned,
        } => {
            if let Some(cache) = model.envs.get_mut(&env)
                && let Some(project) = cache.project_mut(&path)
            {
                project.settings.pinned = prior_pinned;
            }
        }
    }
}

fn restore_active_draft(model: &mut Model, recovery: DraftData, effects: &mut Vec<Effect>) {
    let draft = &mut model.composer.draft;
    prepend_draft(draft, recovery);
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
        text: draft.text.clone(),
        pills: draft.pills.clone(),
    }));
}

fn prepend_draft(draft: &mut DraftData, mut recovery: DraftData) {
    if !recovery.text.is_empty() && !draft.text.is_empty() {
        recovery.text.push_str("\n\n");
    }
    let offset = recovery.text.len();
    recovery.text.push_str(&draft.text);
    recovery.pills.extend(draft.pills.drain(..).map(|mut pill| {
        pill.range.start += offset;
        pill.range.end += offset;
        pill
    }));
    recovery
        .attachment_statuses
        .resize(recovery.attachments.len(), AttachmentStatus::Ready);
    recovery
        .attachment_statuses
        .append(&mut draft.attachment_statuses);
    if !draft.attachments.is_empty() {
        let appended = std::sync::Arc::unwrap_or_clone(std::mem::take(&mut draft.attachments));
        std::sync::Arc::make_mut(&mut recovery.attachments).extend(appended);
    }
    *draft = recovery;
}

pub(super) fn queue_entry_image_attachments(
    model: &mut Model,
    thread: ThreadId,
    entries: &[Entry],
    effects: &mut Vec<Effect>,
) {
    if !model.connected() || model.selection.selected != Some(thread) {
        return;
    }
    let mut ids = Vec::new();
    for entry in entries {
        if let EntryData::Prompt { prompt, .. } = &entry.data {
            for att in prompt.attachments() {
                if att.mime.starts_with("image/") {
                    ids.push(att.id);
                }
            }
        }
    }
    for id in ids {
        let should_fetch = model
            .cache_mut()
            .open_thread_mut(thread)
            .media_slots
            .entry(id.clone())
            .or_default()
            .begin();
        if should_fetch && model.connection.live.request_media(thread, id.clone()) {
            effects.push(Effect::Server(ServerEffect::ReadAttachment {
                thread,
                id,
                preview_request: None,
            }));
        }
    }
}
