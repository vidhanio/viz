//! Apply project RPC results while rejecting stale palette responses.

use viz_client::protocol::CloneState;

use super::{CLONE_INTERVAL, back, browse, open_project, page, page_mut};
use crate::{
    effects::Effect,
    model::{
        Model,
        projects::{Destination, ProjectReply, ProjectView},
    },
    update::{palette, selection::select_thread, server_sync::queue_project_icon},
};

pub(in crate::update) fn reply(
    model: &mut Model,
    request: u64,
    result: Result<ProjectReply, String>,
    effects: &mut Vec<Effect>,
) {
    let valid = if let Some(current) = page_mut(model).filter(|page| page.request == request) {
        current.pending = false;
        current.write_pending = false;
        true
    } else {
        model.palette.open
            && model.palette.pages.stack.is_empty()
            && request == model.palette.pages.request
    };
    let result = match result {
        Ok(result) => result,
        Err(error) if valid => {
            if let Some(current) = page_mut(model) {
                current.error = Some(error);
            } else {
                effects.push(Effect::Toast {
                    title: "Project request failed".into(),
                    detail: Some(error),
                });
            }
            return;
        }
        Err(_) => return,
    };
    match result {
        ProjectReply::Added(project) => {
            queue_project_icon(model, &project, effects);
            model.cache_mut().insert_project_sorted(project.clone());
            if valid {
                if model.palette.open {
                    open_project(model, &project, effects);
                } else {
                    palette::close(model, effects);
                }
            }
        }
        ProjectReply::Updated(project) => {
            refresh_icon(model, &project, effects);
            model.cache_mut().insert_project_sorted(project.clone());
            if valid {
                match page(model).map(|page| &page.view) {
                    Some(ProjectView::Folders(Destination::Add)) => {
                        open_project(model, &project, effects);
                    }
                    Some(
                        ProjectView::Edit { .. }
                        | ProjectView::Folders(Destination::Additional(_))
                        | ProjectView::Color { .. },
                    ) => back(model, effects),
                    _ => {}
                }
            }
        }
        ProjectReply::Removed(project) => {
            model.cache_mut().remove_project(&project);
            if valid {
                palette::close(model, effects);
            }
        }
        ProjectReply::Promoted { project, threads } => {
            model.cache_mut().insert_project_sorted(project.clone());
            queue_project_icon(model, &project, effects);
            for id in threads {
                if let Some(thread) = model.cache_mut().summary_mut(id) {
                    thread.project = Some(project.path.clone());
                }
            }
            if valid {
                palette::close(model, effects);
            }
        }
        _ if !valid => {}
        ProjectReply::Imported(thread) => {
            palette::close(model, effects);
            select_thread(model, thread, effects);
        }
        ProjectReply::Directory(path) => {
            if page(model).is_some_and(|page| page.view == ProjectView::NewFolder) {
                back(model, effects);
            }
            browse(model, path, effects);
        }
        ProjectReply::Clone(status) => {
            if let CloneState::Running { .. } = status.state {
                model.palette.pages.due =
                    Some(model.palette.pages.clock + CLONE_INTERVAL.as_secs_f64());
                effects.push(Effect::WakeAfter(CLONE_INTERVAL));
            }
            if let Some(page) = page_mut(model) {
                page.data = Some(ProjectReply::Clone(status));
            }
        }
        ProjectReply::Sessions(reply) => merge_sessions(model, reply),
        result => {
            if let Some(page) = page_mut(model) {
                page.data = Some(result);
            }
        }
    }
}

fn merge_sessions(model: &mut Model, mut reply: viz_client::protocol::HarnessSessions) {
    if let Some(page) = page_mut(model) {
        if let Some(ProjectReply::Sessions(previous)) = &mut page.data {
            previous
                .sessions
                .retain(|session| !reply.sessions.iter().any(|next| next.id == session.id));
            previous.sessions.append(&mut reply.sessions);
            previous.next = reply.next;
        } else {
            page.data = Some(ProjectReply::Sessions(reply));
        }
    }
}

fn refresh_icon(
    model: &mut Model,
    project: &viz_client::protocol::Project,
    effects: &mut Vec<Effect>,
) {
    if model
        .cache()
        .project(&project.path)
        .is_none_or(|previous| previous.settings.icon != project.settings.icon)
        || matches!(
            project.settings.icon,
            viz_client::protocol::ProjectIcon::Uploaded { .. }
        )
    {
        model.cache_mut().project_icons.remove(&project.path);
        queue_project_icon(model, project, effects);
    }
}
