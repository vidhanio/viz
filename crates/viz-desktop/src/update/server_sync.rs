//! Pure helper transitions for workspace sync, thread view updates, failed
//! send recovery, attachment ingestion, and optimistic rollback.

use viz_client::protocol::{
    Entry, EntryData, EntryId, EntryPage, Lifecycle, Project, ProjectIcon, ThreadId, ThreadState,
    ThreadStatus, ThreadSummary, attachment_inline_md, attachment_ref_labels,
};

use super::selection::{clear_selection, focus_draft, select_thread, show_thread};
use crate::{
    effects::{ATTACHMENT_ENCODED_LIMIT, EditSync, Effect, LoadedAttachments, ServerEffect},
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
            thread: thread.id,
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
    let open = model.cache_mut().open_thread_mut(thread);
    let status_changed = open.state.as_ref().map(|prev| prev.status) != Some(state.status);
    open.transcript.sync_acp(state.live.as_ref());
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
    entries: Vec<Entry>,
    reset: bool,
    before: Option<EntryId>,
    effects: &mut Vec<Effect>,
) {
    if reset {
        model.cache_mut().retain_open_thread(thread);
    }
    let open = model.cache_mut().open_thread_mut(thread);
    if reset {
        open.entries.clear();
        open.transcript = Transcript::from_entries(entries.iter(), before);
        open.loading = false;
        open.history_loading = false;
    } else {
        open.transcript.append(entries.iter());
    }
    if !entries.is_empty() {
        queue_entry_image_attachments(model, thread, &entries, effects);
        model
            .cache_mut()
            .open_thread_mut(thread)
            .entries
            .push_back(entries);
    }
    let open = model.cache_mut().open_thread_mut(thread);
    let live = open.state.as_ref().and_then(|state| state.live.as_ref());
    open.transcript.sync_acp(live);
    if model.selection.selected == Some(thread) {
        super::palette::continue_message_jump(model, effects);
    }
}

pub(super) fn apply_earlier_loaded(
    model: &mut Model,
    thread: ThreadId,
    before: EntryId,
    page: Option<EntryPage>,
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
    if open.transcript.before != Some(before) {
        return;
    }
    open.history_loading = false;
    if let Some(page) = page {
        if let Some(first_key) = first_key {
            effects.push(Effect::KeepOnScreen(first_key));
        }
        queue_entry_image_attachments(model, thread, &page.entries, effects);
        let open = model.cache_mut().open_thread_mut(thread);
        open.entries.push_front(page.entries);
        open.transcript = Transcript::from_entries(open.entries.iter().flatten(), page.before);
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
    error: String,
    effects: &mut Vec<Effect>,
) {
    if model.selection.selected == Some(failed) {
        if let Some(previous) = previous {
            show_thread(model, previous, effects);
            model.cache_mut().open_thread_mut(previous).history_loading = false;
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
                .composer
                .drafts_by_env
                .entry(recovery.environment)
                .or_default()
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
    if scope != model.connection.epoch || !model.composer.attachment_reads.remove(&target) {
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
                    .composer
                    .drafts_by_env
                    .entry(env)
                    .or_default()
                    .entry(id)
                    .or_default(),
            ),
        };
        let existing = std::sync::Arc::make_mut(&mut draft.attachments);
        let prior_len = existing.len();
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
        if existing.len() > prior_len {
            let labels = attachment_ref_labels(existing.iter().map(|file| file.mime.as_str()));
            let snippet = existing[prior_len..]
                .iter()
                .zip(&labels[prior_len..])
                .map(|(file, label)| attachment_inline_md(&file.name, &file.mime, label))
                .collect::<Vec<_>>()
                .join(" ");
            if is_active {
                effects.push(Effect::SyncEdit(EditSync::InsertAtDraftCaret(snippet)));
            } else {
                if !draft.text.is_empty() && !draft.text.ends_with(char::is_whitespace) {
                    draft.text.push(' ');
                }
                draft.text.push_str(&snippet);
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
        OptimisticUndo::ProjectPin {
            env,
            path,
            prior_pinned,
        } => {
            if let Some(cache) = model.caches.get_mut(&env)
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

fn queue_entry_image_attachments(
    model: &mut Model,
    thread: ThreadId,
    entries: &[Entry],
    effects: &mut Vec<Effect>,
) {
    if !model.connected() {
        return;
    }
    for entry in entries {
        if let EntryData::Prompt { attachments, .. } = &entry.data {
            for att in attachments {
                if !att.mime.starts_with("image/") {
                    continue;
                }
                let key = (thread, att.id.clone());
                if !model.timeline.loaded_attachments.contains_key(&key)
                    && model.timeline.pending_attachments.insert(key)
                {
                    effects.push(Effect::Server(ServerEffect::ReadAttachment {
                        thread,
                        id: att.id.clone(),
                        preview_request: None,
                    }));
                }
            }
        }
    }
}
