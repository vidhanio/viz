//! Pure navigation and request lifecycle for project palette pages.

mod reply;
mod settings;
use std::{path::Path, time::Duration};

pub(super) use reply::reply;
use viz_client::protocol::{
    BrowseFolders, CloneState, GitHubRepositories, ListHarnessSessions, Project,
    ProjectSettingsPatch, StartClone,
};

use super::{
    palette,
    selection::{focus_draft, open_in_environment, prepare_draft, select_thread},
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect, projects::ProjectRequest},
    model::{
        Model, NewThread,
        projects::{Destination, ProjectAction, ProjectPage, ProjectReply, ProjectView},
    },
};

const LOOKUP_DELAY: Duration = Duration::from_millis(180);
const CLONE_INTERVAL: Duration = Duration::from_millis(350);

pub(super) fn push(model: &mut Model, view: ProjectView, query: String, effects: &mut Vec<Effect>) {
    model
        .palette
        .pages
        .stack
        .push(ProjectPage::new(view, query));
    sync(model, false, effects);
    refresh(model, effects);
}

pub(super) fn back(model: &mut Model, effects: &mut Vec<Effect>) {
    if page(model).is_some_and(|page| {
        page.pending || matches!(page.view, ProjectView::Cloning) && clone_running(page)
    }) {
        return;
    }
    if model.palette.pages.stack.pop().is_some() {
        sync(model, true, effects);
    }
}

fn sync(model: &mut Model, backwards: bool, effects: &mut Vec<Effect>) {
    let query = page(model)
        .map_or(&model.palette.query, |page| &page.query)
        .clone();
    let pages = &mut model.palette.pages;
    pages.revision = pages.revision.wrapping_add(1);
    pages.backwards = backwards;
    pages.due = None;
    effects.extend([
        Effect::SyncEdit(EditSync::Replace {
            target: EditTarget::PaletteQuery,
            text: query,
            select_all: false,
        }),
        Effect::Focus {
            key: "palette-query",
            select_all: false,
            when_shown: true,
        },
    ]);
}

pub(super) fn page(model: &Model) -> Option<&ProjectPage> {
    model.palette.pages.stack.last()
}

fn page_mut(model: &mut Model) -> Option<&mut ProjectPage> {
    model.palette.pages.stack.last_mut()
}

fn send(model: &mut Model, command: ProjectRequest, effects: &mut Vec<Effect>) {
    let pages = &mut model.palette.pages;
    pages.request = pages.request.wrapping_add(1);
    if let Some(page) = pages.stack.last_mut() {
        page.request = pages.request;
        page.pending = true;
        page.write_pending = !matches!(
            command,
            ProjectRequest::Browse(_)
                | ProjectRequest::Repositories(_)
                | ProjectRequest::Sessions(_)
                | ProjectRequest::CloneStatus(_)
        );
        page.error = None;
    }
    effects.push(Effect::Server(ServerEffect::ProjectPalette(
        pages.request,
        command,
    )));
}

pub(super) fn query(model: &mut Model, text: String, clock: f64, effects: &mut Vec<Effect>) {
    let Some(current) = page_mut(model) else {
        return;
    };
    current.query = text;
    current.active = None;
    if matches!(
        current.view,
        ProjectView::Folders(_) | ProjectView::Repositories
    ) {
        current.request = 0;
        current.data = None;
        current.error = None;
        current.pending = true;
        model.palette.pages.due = Some(clock + LOOKUP_DELAY.as_secs_f64());
        effects.push(Effect::WakeAfter(LOOKUP_DELAY));
    }
}

pub(super) fn poll(model: &mut Model, clock: f64, effects: &mut Vec<Effect>) {
    model.palette.pages.clock = clock;
    if model.palette.pages.due.is_some_and(|due| clock >= due) {
        model.palette.pages.due = None;
        if !page(model).is_some_and(|page| page.view == ProjectView::Cloning) {
            refresh(model, effects);
        }
    }
    if let Some(current) = page(model)
        && matches!(current.view, ProjectView::Cloning)
        && clone_running(current)
        && !current.pending
        && current.error.is_none()
    {
        let Some(ProjectReply::Clone(status)) = &current.data else {
            return;
        };
        send(
            model,
            ProjectRequest::CloneStatus(status.id.clone()),
            effects,
        );
    }
}

fn refresh(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(current) = page(model) else { return };
    let command = match &current.view {
        ProjectView::Folders(destination) => ProjectRequest::Browse(BrowseFolders {
            path: (!current.query.is_empty()).then(|| current.query.clone()),
            project: match destination {
                Destination::Additional(project) => Some(project.clone()),
                _ if current.query.starts_with('.') => model.project_path().map(str::to_owned),
                _ => None,
            },
            show_hidden: current.show_hidden,
        }),
        ProjectView::Repositories => ProjectRequest::Repositories(GitHubRepositories {
            host: None,
            query: (!current.query.trim().is_empty()).then(|| current.query.trim().to_owned()),
            page: None,
        }),
        ProjectView::Sessions { harness, project } => {
            ProjectRequest::Sessions(ListHarnessSessions {
                harness: harness.clone(),
                project: project.clone(),
                cursor: None,
            })
        }
        _ => return,
    };
    send(model, command, effects);
}

pub(super) fn clone_active(model: &Model) -> bool {
    page(model).is_some_and(|page| {
        page.view == ProjectView::Cloning && (page.pending || clone_running(page))
    })
}

pub(super) const fn clone_running(page: &ProjectPage) -> bool {
    matches!(&page.data, Some(ProjectReply::Clone(status)) if matches!(status.state, CloneState::Running { .. }))
}

pub(super) fn action(model: &mut Model, action: ProjectAction, effects: &mut Vec<Effect>) {
    let Some(current) = page(model) else { return };
    if current.pending && (current.write_pending || !matches!(action, ProjectAction::CancelClone)) {
        return;
    }
    match action {
        ProjectAction::Push(view, query) => push(model, view, query, effects),
        ProjectAction::Browse(path) => browse(model, path, effects),
        ProjectAction::Hidden => {
            if let Some(page) = page_mut(model) {
                page.show_hidden = !page.show_hidden;
            }
            model.palette.pages.due = None;
            refresh(model, effects);
        }
        ProjectAction::Init => {
            if let Some(ProjectReply::Folders(folders)) = &current.data {
                send(model, ProjectRequest::Init(folders.path.clone()), effects);
            }
        }
        ProjectAction::SubmitFolder => submit_folder(model, effects),
        ProjectAction::Submit => submit(model, effects),
        ProjectAction::Save(project, patch) => send(
            model,
            ProjectRequest::Save {
                project,
                name: None,
                patch,
            },
            effects,
        ),
        ProjectAction::Import(args) => send(model, ProjectRequest::Import(args), effects),
        ProjectAction::Open(thread) => {
            palette::close(model, effects);
            select_thread(model, thread, effects);
        }
        ProjectAction::CancelClone => {
            if let Some(ProjectReply::Clone(status)) = &current.data {
                send(
                    model,
                    ProjectRequest::CancelClone(status.id.clone()),
                    effects,
                );
            }
        }
        ProjectAction::MoreSessions => {
            if let (
                ProjectView::Sessions { harness, project },
                Some(ProjectReply::Sessions(result)),
            ) = (&current.view, &current.data)
            {
                send(
                    model,
                    ProjectRequest::Sessions(ListHarnessSessions {
                        harness: harness.clone(),
                        project: project.clone(),
                        cursor: result.next.clone(),
                    }),
                    effects,
                );
            }
        }
        ProjectAction::Switch(environment) => {
            palette::close(model, effects);
            open_in_environment(model, environment, None, effects);
        }
        ProjectAction::UploadIcon(project) => {
            send(model, ProjectRequest::UploadIcon(project), effects);
        }
    }
}

pub(super) fn highlight(model: &mut Model, index: usize, effects: &mut Vec<Effect>) {
    if let Some(page) = page_mut(model) {
        page.active = Some(index);
        effects.push(Effect::ScrollIntoView {
            list: "palette-results",
            item: format!("project-option-{index}"),
        });
    }
}

fn browse(model: &mut Model, path: String, effects: &mut Vec<Effect>) {
    if let Some(current) = page_mut(model) {
        current.query = path;
        current.data = None;
        current.active = None;
    }
    sync(model, false, effects);
    refresh(model, effects);
}

fn submit_folder(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(current) = page(model) else { return };
    let (ProjectView::Folders(destination), Some(ProjectReply::Folders(folders))) =
        (&current.view, &current.data)
    else {
        return;
    };
    if current.pending || folders.issue.is_some() {
        return;
    }
    let path = folders.path.clone();
    match destination {
        Destination::Add if folders.project => {
            if let Some(project) = model.cache().project(&path).cloned() {
                if project.settings.archived {
                    send(
                        model,
                        ProjectRequest::Save {
                            project: path,
                            name: None,
                            patch: Box::new(ProjectSettingsPatch {
                                archived: Some(false),
                                ..Default::default()
                            }),
                        },
                        effects,
                    );
                } else {
                    open_project(model, &project, effects);
                }
            } else {
                send(model, ProjectRequest::Add(path), effects);
            }
        }
        Destination::Add => send(model, ProjectRequest::Add(path), effects),
        Destination::Additional(project) => {
            let Some(settings) = model
                .cache()
                .project(project)
                .map(|project| &project.settings)
            else {
                return;
            };
            let mut directories = settings.additional_directories.clone();
            if !directories.contains(&path) {
                directories.push(path);
            }
            let project = project.clone();
            send(
                model,
                ProjectRequest::Save {
                    project,
                    name: None,
                    patch: Box::new(ProjectSettingsPatch {
                        additional_directories: Some(directories),
                        ..Default::default()
                    }),
                },
                effects,
            );
        }
        Destination::Clone { url, name } => {
            let args = StartClone {
                url: url.clone(),
                path: Path::new(&path).join(name).to_string_lossy().into_owned(),
                name: None,
            };
            push(model, ProjectView::Cloning, String::new(), effects);
            send(model, ProjectRequest::Clone(args), effects);
        }
    }
}

fn submit(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(current) = page(model) else { return };
    let value = current.query.trim();
    let command = match &current.view {
        ProjectView::CloneUrl => {
            if let Some(next) = crate::model::projects::clone_destination(value.to_owned()) {
                action(model, next, effects);
            } else if let Some(page) = page_mut(model) {
                page.error = Some("Enter a Git clone URL with a repository name.".into());
            }
            return;
        }
        ProjectView::NewFolder => {
            let Some(parent) = model
                .palette
                .pages
                .stack
                .iter()
                .rev()
                .nth(1)
                .and_then(|page| match &page.data {
                    Some(ProjectReply::Folders(data)) => Some(&data.path),
                    _ => None,
                })
            else {
                return;
            };
            if value.is_empty() || value.contains(['/', '\\']) || matches!(value, "." | "..") {
                if let Some(page) = page_mut(model) {
                    page.error = Some("Enter a single folder name.".into());
                }
                return;
            }
            ProjectRequest::CreateFolder(
                Path::new(parent).join(value).to_string_lossy().into_owned(),
            )
        }
        ProjectView::Edit { project, field } => match settings::edit(project.clone(), field, value)
        {
            Ok(command) => command,
            Err(error) => {
                if let Some(page) = page_mut(model) {
                    page.error = Some(error);
                }
                return;
            }
        },
        ProjectView::Remove(project) => ProjectRequest::Remove(project.clone()),
        _ => return,
    };
    send(model, command, effects);
}

fn open_project(model: &mut Model, project: &Project, effects: &mut Vec<Effect>) {
    palette::close(model, effects);
    if let Some(draft) = model
        .selection
        .new_thread
        .as_mut()
        .filter(|draft| !draft.sending)
    {
        draft.set_project(Some(project));
        effects.push(focus_draft());
        return;
    }
    let thread = model
        .cache()
        .threads()
        .find(|thread| {
            thread.project.as_deref() == Some(project.path.as_str()) && !thread.lifecycle.archived
        })
        .map(|thread| thread.id);
    if let Some(thread) = thread {
        select_thread(model, thread, effects);
    } else {
        prepare_draft(
            model,
            NewThread::new(model.selection.next_draft, Some(project)),
            effects,
        );
    }
}

pub(super) fn promote(model: &mut Model, effects: &mut Vec<Effect>) {
    if let Some(thread) = model.selection.selected {
        send(
            model,
            ProjectRequest::Promote(viz_client::protocol::PromoteThread { thread, name: None }),
            effects,
        );
    }
}
