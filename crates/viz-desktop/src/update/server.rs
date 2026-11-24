//! Pure transitions for server responses, push notifications, and optimistic
//! rollbacks.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use viz_client::protocol::{ConfigOption, Harness, ThreadId};

pub use super::server_msg::ServerMsg;
use super::{
    selection::{clear_selection, focus_draft, show_thread},
    server_sync::{
        apply_attachments_loaded, apply_earlier_loaded, apply_launch_failed,
        apply_project_icon_loaded, apply_select_failed, apply_send_failed, apply_thread_created,
        apply_thread_state, apply_thread_triaged, apply_thread_view, apply_workspace_sync,
        is_active_env, rollback_optimistic,
    },
    timeline::{apply_entry_chunk_loaded, apply_thread_forked},
};
use crate::{
    effects::Effect,
    model::{Model, PaletteSearch, SessionId, Status},
};

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
            if matches!(status, Status::Error(_)) {
                model.connection.session = SessionId(0);
                model.timeline.clear_in_flight();
            }
            model.connection.status = status;
            effects.push(Effect::SetWindowTitle(model.window_title()));
        }
        ServerMsg::Connected {
            session,
            version,
            environment,
            environments,
            harnesses,
            projects,
            threads,
            tasks,
            watched,
        } => {
            if model
                .connection
                .environment
                .as_deref()
                .is_some_and(|expected| expected != environment)
            {
                return effects;
            }
            model.connection.session = session;
            model.timeline.clear_in_flight();
            model.connection.status = Status::Connected { version };
            model.connection.environment = Some(environment);
            model.connection.environments = environments;
            refresh_harnesses(model, harnesses);
            if model.selection.new_thread.is_none() {
                if let Some(thread) = watched {
                    show_thread(model, thread, &mut effects);
                    effects.push(focus_draft());
                } else {
                    clear_selection(model, &mut effects);
                }
            }
            apply_workspace_sync(model, projects, threads, &mut effects);
            model.cache_mut().tasks = tasks;
            effects.push(Effect::SetWindowTitle(model.window_title()));
        }
        ServerMsg::RequestAnswered { request, result } => {
            return super::requests::update(
                model,
                super::requests::RequestMsg::Finished(request, result),
            );
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
            refresh_harnesses(model, harnesses);
        }
        ServerMsg::ThreadState { thread, state } => {
            apply_thread_state(model, thread, *state, &mut effects);
        }
        ServerMsg::ThreadLive { thread, delta } => {
            let open = model.cache_mut().open_thread_mut(thread);
            if let Some(state) = &mut open.state {
                delta.apply(&mut state.live);
                open.transcript.sync_acp(state.live.as_ref());
            }
            if model.selection.selected == Some(thread) && model.timeline.far_from_bottom {
                model.timeline.unseen_activity = true;
            }
        }
        ServerMsg::ThreadView {
            thread,
            entries,
            reset,
            before,
        } => {
            if !reset && model.selection.selected == Some(thread) && model.timeline.far_from_bottom
            {
                model.timeline.unseen_activity = true;
            }
            apply_thread_view(model, thread, entries, reset, before, &mut effects);
        }
        ServerMsg::EarlierLoaded {
            thread,
            before,
            page,
        } => {
            apply_earlier_loaded(model, thread, before, page, &mut effects);
        }
        ServerMsg::SelectFailed {
            failed,
            previous,
            error,
        } => apply_select_failed(model, failed, previous, error, &mut effects),
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
                Some(Ok(harnesses)) => refresh_harnesses(model, harnesses),
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
            model: requested_model,
            result,
        } => {
            if model.connection.environment.as_deref() != Some(environment.as_str()) {
                return effects;
            }
            let current = crate::model::picker_scope(model)
                .is_some_and(|scope| scope.harness == harness)
                && model.composer.model_menu.loading_levels.as_deref()
                    == Some(requested_model.as_str());
            match result {
                Ok(options) => {
                    if let Some(runner) = model
                        .cache_mut()
                        .harnesses
                        .iter_mut()
                        .find(|runner| runner.id == harness)
                    {
                        retain_model_effort(&mut runner.options, &options);
                    }
                    if current {
                        model.composer.model_menu.loading_levels = None;
                    }
                }
                Err(error) if current => {
                    model.composer.model_menu.loading_levels = None;
                    effects.push(Effect::Toast {
                        title: "Couldn’t load model options".into(),
                        detail: Some(error),
                    });
                }
                Err(_) => {}
            }
        }
        ServerMsg::ThreadCreated { draft, thread } => {
            apply_thread_created(model, draft, thread, &mut effects);
        }
        ServerMsg::LaunchFailed {
            draft,
            recovery,
            title,
            detail,
        } => apply_launch_failed(model, draft, *recovery, title, detail, &mut effects),
        ServerMsg::SendFailed {
            thread,
            recovery,
            title,
            detail,
        } => apply_send_failed(
            model,
            thread,
            recovery.map(|recovery| *recovery),
            title,
            detail,
            &mut effects,
        ),
        ServerMsg::ProjectPalette {
            environment,
            request,
            result,
        } => {
            if model.connection.environment.as_deref().unwrap_or_default() == environment {
                super::projects::reply(model, request, *result, &mut effects);
            }
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
        ServerMsg::FileSearchResult { request, result } => {
            if model.composer.completion.request == request {
                model.composer.completion.files_query = model
                    .composer
                    .completion
                    .token
                    .as_ref()
                    .map(|token| token.query.clone());
                model.composer.completion.files = Some(result);
            }
        }
        ServerMsg::GitHubSearchResult { request, result } => {
            if model.composer.completion.request == request {
                model.composer.completion.github = Some(result);
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
        ServerMsg::ThreadForked {
            forked,
            load_prompt,
        } => apply_thread_forked(model, *forked, load_prompt, &mut effects),
        ServerMsg::EntryChunkLoaded {
            thread,
            key,
            source,
            result,
        } => apply_entry_chunk_loaded(model, thread, &key, &source, result, &mut effects),
        ServerMsg::FilePreviewLoaded { request, result } => {
            apply_file_preview_loaded(model, request, result, &mut effects);
        }
        ServerMsg::ThreadAttachmentLoaded {
            thread,
            id,
            preview_request,
            result,
        } => {
            apply_thread_attachment_loaded(
                model,
                thread,
                id,
                preview_request,
                result,
                &mut effects,
            );
        }
    }
    effects
}

fn apply_file_preview_loaded(
    model: &mut Model,
    request: u64,
    result: Result<viz_client::protocol::FileContent, String>,
    effects: &mut Vec<Effect>,
) {
    let Some(preview) = model.timeline.file_preview.as_mut() else {
        return;
    };
    if preview.request != request {
        return;
    }
    match result {
        Ok(content) => {
            preview.path.clone_from(&content.path);
            match content.body {
                viz_client::protocol::FileBody::Text {
                    text,
                    start_line,
                    truncated,
                    ..
                } => {
                    let target_line = preview.lines.map(|(start, _)| start);
                    preview.body = crate::model::FilePreviewBody::Text {
                        text,
                        start_line: usize::try_from(start_line).unwrap_or(1).max(1),
                        truncated,
                        size: content.size,
                    };
                    if let Some(start) = target_line {
                        effects.push(Effect::ScrollCenterInstant {
                            list: crate::timeline::file_preview::PREVIEW_SCROLL_KEY,
                            item: format!("preview-line-{start}"),
                        });
                    }
                }
                viz_client::protocol::FileBody::Image { data } => {
                    if let Ok(bytes) = STANDARD.decode(&data) {
                        preview.body = crate::model::FilePreviewBody::Image { size: content.size };
                        effects.push(Effect::DecodePreviewImage {
                            media_type: content.mime,
                            bytes,
                        });
                    } else {
                        preview.body = crate::model::FilePreviewBody::Error(
                            "Couldn’t decode image bytes".into(),
                        );
                    }
                }
                viz_client::protocol::FileBody::Binary { .. } => {
                    preview.body = crate::model::FilePreviewBody::Binary {
                        mime: content.mime,
                        size: content.size,
                    };
                }
            }
        }
        Err(error) => {
            preview.body = crate::model::FilePreviewBody::Error(error);
        }
    }
}

fn apply_thread_attachment_loaded(
    model: &mut Model,
    thread: ThreadId,
    id: String,
    preview_request: Option<u64>,
    result: Result<viz_client::protocol::Attachment, String>,
    effects: &mut Vec<Effect>,
) {
    let env = model.active_env_key().to_owned();
    model
        .timeline
        .pending_attachments
        .remove(&(env.clone(), thread, id.clone()));
    match result {
        Ok(attachment) => {
            if attachment.mime.starts_with("image/")
                && let Ok(bytes) = STANDARD.decode(&attachment.data)
            {
                effects.push(Effect::DecodeThreadAttachmentImage {
                    environment: env.clone(),
                    thread,
                    id: id.clone(),
                    media_type: attachment.mime.clone(),
                    bytes,
                });
            }
            if let Some(req) = preview_request
                && model
                    .timeline
                    .file_preview
                    .as_ref()
                    .is_some_and(|p| p.request == req)
            {
                super::timeline::apply_loaded_attachment_to_preview(
                    model,
                    req,
                    &attachment,
                    effects,
                );
            }
            model
                .timeline
                .loaded_attachments
                .insert((env, thread, id), attachment);
        }
        Err(error) => {
            if let Some(req) = preview_request
                && let Some(preview) = model.timeline.file_preview.as_mut()
                && preview.request == req
            {
                preview.body = crate::model::FilePreviewBody::Error(error);
            }
        }
    }
}

/// Refreshes harness metadata while keeping profiles learned by model-specific
/// requests.
fn refresh_harnesses(model: &mut Model, mut harnesses: Vec<Harness>) {
    for runner in &mut harnesses {
        if let Some(previous) = model.harnesses().iter().find(|old| old.id == runner.id) {
            retain_model_effort(&mut runner.options, &previous.options);
        }
    }
    model.cache_mut().harnesses = harnesses;
}

/// Fills absent model effort profiles without changing the catalog's selected
/// defaults or the draft's explicit option overrides.
fn retain_model_effort(target: &mut [ConfigOption], source: &[ConfigOption]) {
    for option in target {
        let Some(previous) = source.iter().find(|previous| previous.id == option.id) else {
            continue;
        };
        for choice in option
            .choices
            .iter_mut()
            .filter(|choice| choice.thinking.is_none())
        {
            if let Some(prior) = previous
                .choices
                .iter()
                .find(|prior| prior.value == choice.value)
            {
                choice.thinking.clone_from(&prior.thinking);
            }
        }
    }
}
