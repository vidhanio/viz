//! Pure Git interaction and request coordination.

use viz_client::protocol::{GitAction, GitDiffTarget, GitStatus, ThreadId, WorktreeTarget};

use crate::{
    effects::{
        Effect, ServerEffect,
        git::{Reply, Request},
    },
    model::{
        Model,
        git::{Dialog, GitState, Popup},
    },
};

#[derive(Debug, Clone)]
pub enum GitMsg {
    ToggleDetails,
    RefreshDiff,
    ToggleDiff,
    PaneWidth(f32),
    Popup(Option<Popup>),
    Search(String),
    MoreBranches,
    HighlightBranch(String),
    NewWorktree,
    RemoteBase,
    ChooseBranch {
        name: String,
        upstream: Option<String>,
        create: bool,
    },
    Dialog(Option<Dialog>),
    Exclude(String),
    Scope(GitDiffTarget),
    Request(Request),
    Status(ThreadId, Option<GitStatus>),
    Reply {
        thread: ThreadId,
        serial: u64,
        result: Result<Reply, String>,
    },
}

pub(super) fn reconcile(model: &mut Model, effects: &mut Vec<Effect>) {
    let next = model
        .selection
        .selected
        .filter(|_| model.connected())
        .and_then(|id| {
            let project = model.project().filter(|project| project.git)?;
            let checkout = model
                .thread_state()
                .map(|state| state.cwd.as_str())
                .or_else(|| {
                    model
                        .summary(id)?
                        .worktree
                        .as_ref()
                        .map(|worktree| worktree.path.as_str())
                })
                .unwrap_or(&project.path);
            Some((id, checkout.to_owned()))
        });
    if next == model.git.watch {
        return;
    }
    let thread = next
        .as_ref()
        .or(model.git.watch.as_ref())
        .map(|(id, _)| *id);
    let serial = model.git.serial + 1;
    model.git = GitState {
        watch: next,
        serial,
        ..GitState::default()
    };
    if let Some(thread) = thread.filter(|_| model.connected()) {
        effects.push(Effect::Server(ServerEffect::Git {
            thread,
            serial,
            request: Request::Watch(model.git.watch.as_ref().map(|(id, _)| *id)),
        }));
    }
    if model.panels.diff.open && model.git.watch.is_none() {
        model.panels.diff.open = false;
        effects.push(Effect::SyncDiffPane {
            open: Some(false),
            width: None,
        });
    }
}

fn send(model: &mut Model, request: Request, effects: &mut Vec<Effect>) {
    let Some(thread) = model.selection.selected.filter(|_| model.connected()) else {
        return;
    };
    let serial = model.git.serial + 1;
    match &request {
        Request::Branches { .. } => model.git.branch_request = serial,
        Request::Diff(_) => model.git.diff_request = serial,
        Request::Action(_)
        | Request::CreateCheckout { .. }
        | Request::Init(_)
        | Request::CreatePull { .. }
        | Request::Restore { .. } => {
            if model.busy() || model.git.busy {
                return;
            }
            model.git.busy = true;
            model.git.mutation_request = serial;
            model.git.error = None;
        }
        _ => model.git.overlay_request = serial,
    }
    model.git.serial = serial;
    effects.push(Effect::Server(ServerEffect::Git {
        thread,
        serial,
        request,
    }));
}

fn branches(model: &mut Model, offset: Option<usize>, effects: &mut Vec<Effect>) {
    if let Some(project) = model.project() {
        send(
            model,
            Request::Branches {
                project: project.path.clone(),
                query: model.git.query.clone(),
                offset,
            },
            effects,
        );
    }
}

fn refresh_diff(model: &mut Model, effects: &mut Vec<Effect>) {
    if model.panels.diff.open && model.git.status.is_some() {
        send(model, Request::Diff(model.git.target.clone()), effects);
    }
}

fn choose_branch(
    model: &mut Model,
    name: String,
    upstream: Option<String>,
    create: bool,
    effects: &mut Vec<Effect>,
) {
    let Some(thread) = model
        .selection
        .selected
        .filter(|_| model.connected() && !model.busy() && !model.git.busy)
    else {
        return;
    };
    let remote_base = model.git.remote_base;
    let new_worktree = std::mem::take(&mut model.git.new_worktree);
    model.git.popup = None;
    if new_worktree {
        let base = if create {
            model
                .git
                .status
                .as_ref()
                .and_then(|status| status.branch.clone())
        } else if remote_base {
            Some(upstream.unwrap_or_else(|| {
                if name.contains('/')
                    && model.git.branches.as_ref().is_some_and(|page| {
                        page.branches
                            .iter()
                            .any(|branch| branch.name == name && branch.remote)
                    })
                {
                    name.clone()
                } else {
                    format!("origin/{name}")
                }
            }))
        } else {
            Some(name.clone())
        };
        effects.push(Effect::Server(ServerEffect::Move {
            thread,
            worktree: WorktreeTarget::New {
                base,
                branch: create.then_some(name),
            },
        }));
    } else if create {
        send(
            model,
            Request::CreateCheckout {
                name,
                start: "HEAD".into(),
            },
            effects,
        );
    } else if model.git.branches.as_ref().is_some_and(|page| {
        page.branches
            .iter()
            .any(|branch| branch.name == name && branch.remote)
    }) {
        if let Some((_, local)) = name.split_once('/') {
            send(
                model,
                Request::CreateCheckout {
                    name: local.to_owned(),
                    start: name,
                },
                effects,
            );
        }
    } else if let Some(path) = model
        .project()
        .and_then(|project| model.checkouts(&project.path))
        .and_then(|checkouts| {
            checkouts
                .worktrees
                .iter()
                .find(|w| w.branch.as_deref() == Some(&name))
        })
        .map(|w| w.path.clone())
        .filter(|path| {
            model
                .git
                .status
                .as_ref()
                .is_some_and(|s| s.checkout != *path)
        })
    {
        let worktree = if model.project_path() == Some(&path) {
            WorktreeTarget::Local
        } else {
            WorktreeTarget::Existing { path }
        };
        effects.push(Effect::Server(ServerEffect::Move { thread, worktree }));
    } else {
        send(
            model,
            Request::Action(GitAction::SwitchBranch { name }),
            effects,
        );
    }
}

fn accept_reply(
    model: &mut Model,
    serial: u64,
    result: Result<Reply, String>,
    effects: &mut Vec<Effect>,
) {
    let current = [
        model.git.branch_request,
        model.git.diff_request,
        model.git.overlay_request,
        model.git.mutation_request,
    ];
    if !current.contains(&serial) {
        return;
    }
    let mutation = serial == model.git.mutation_request;
    if mutation {
        model.git.busy = false;
    }
    match result {
        Ok(Reply::Branches { mut page, append }) => {
            if append && let Some(previous) = model.git.branches.take() {
                let mut all = previous.branches;
                all.append(&mut page.branches);
                page.branches = all;
            }
            model.git.default_branch.clone_from(&page.default);
            model.git.branches = Some(page);
        }
        Ok(Reply::Diff(diff)) => {
            model.git.diff = Some(diff.into());
            model.git.diff_error = None;
            if let Some(path) = model.timeline.selected_diff_file.clone() {
                effects.push(Effect::RevealDiffFile(path));
            }
        }
        Ok(Reply::Preview(diff)) => model.git.preview = Some(diff),
        Ok(Reply::Checks(checks)) => model.git.checks = Some(checks),
        Ok(Reply::Reviews(reviews)) => model.git.reviews = Some(reviews),
        Ok(Reply::Done) => {
            model.git.error = None;
            model.git.dialog = None;
            effects.push(Effect::Toast {
                title: "Git action completed".into(),
                detail: None,
            });
            refresh_diff(model, effects);
        }
        Err(error) if serial == model.git.diff_request => model.git.diff_error = Some(error),
        Err(error) => {
            model.git.error = Some(error.clone());
            effects.push(Effect::Toast {
                title: "Git request failed".into(),
                detail: Some(error),
            });
        }
    }
}

#[expect(clippy::too_many_lines, reason = "typed workspace event dispatch")]
pub(super) fn update(model: &mut Model, msg: GitMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        GitMsg::RefreshDiff => refresh_diff(model, &mut effects),
        GitMsg::ToggleDetails => {
            model.panels.details_open = !model.panels.details_open;
            model.git.popup = None;
        }
        GitMsg::ToggleDiff => {
            model.panels.diff.open = !model.panels.diff.open;
            if model.panels.diff.open {
                model.panels.details_open = false;
            }
            model.git.popup = None;
            effects.push(Effect::SyncDiffPane {
                open: Some(model.panels.diff.open),
                width: None,
            });
            refresh_diff(model, &mut effects);
        }
        GitMsg::PaneWidth(width) => {
            model.panels.diff.width = width
                .clamp(model.panels.diff.min, model.panels.diff.max)
                .round();
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
        GitMsg::Popup(popup) => {
            model.git.popup = popup;
            model.git.error = None;
            if popup == Some(Popup::Branch) {
                model.git.query.clear();
                model.git.branch_highlight = None;
                model.git.branches = None;
                branches(model, None, &mut effects);
            } else {
                model.git.new_worktree = false;
            }
        }
        GitMsg::HighlightBranch(key) => model.git.branch_highlight = Some(key),
        GitMsg::Search(query) => {
            model.git.branch_highlight = None;
            model.git.query = query;
            model.git.branches = None;
            branches(model, None, &mut effects);
        }
        GitMsg::MoreBranches => {
            if let Some(offset) = model
                .git
                .branches
                .as_mut()
                .and_then(|page| page.next.take())
            {
                branches(model, Some(offset), &mut effects);
            }
        }
        GitMsg::NewWorktree => {
            model.git.new_worktree = true;
            model.git.popup = Some(Popup::Branch);
            model.git.query.clear();
            model.git.branch_highlight = None;
            model.git.branches = None;
            branches(model, None, &mut effects);
        }
        GitMsg::RemoteBase => model.git.remote_base = !model.git.remote_base,
        GitMsg::ChooseBranch {
            name,
            upstream,
            create,
        } => choose_branch(model, name, upstream, create, &mut effects),
        GitMsg::Dialog(dialog) => {
            model.git.popup = None;
            model.git.dialog = dialog;
            model.git.error = None;
            if dialog == Some(Dialog::Commit) {
                model.git.excluded.clear();
                model.git.preview = None;
                send(model, Request::Preview, &mut effects);
            }
        }
        GitMsg::Exclude(path) => {
            if !model.git.excluded.remove(&path) {
                model.git.excluded.insert(path);
            }
        }
        GitMsg::Scope(target) => {
            model.git.target = target;
            model.git.diff = None;
            model.git.diff_error = None;
            model.git.popup = None;
            refresh_diff(model, &mut effects);
        }
        GitMsg::Request(request) => send(model, request, &mut effects),
        GitMsg::Status(thread, status)
            if model
                .git
                .watch
                .as_ref()
                .is_some_and(|(id, _)| *id == thread) =>
        {
            let changed = model.git.status != status;
            let initial = model.git.status.is_none() && status.is_some();
            model.git.status = status;
            if initial {
                branches(model, None, &mut effects);
                if let Some(project) = model.project() {
                    effects.push(Effect::Server(ServerEffect::LoadCheckouts(
                        project.path.clone(),
                    )));
                }
            }
            if model.git.status.is_none() {
                model.git.diff = None;
                model.git.diff_request = 0;
            }
            if changed
                && matches!(
                    model.git.target,
                    GitDiffTarget::Working | GitDiffTarget::Staged | GitDiffTarget::Base { .. }
                )
            {
                refresh_diff(model, &mut effects);
            }
        }
        GitMsg::Reply {
            thread,
            serial,
            result,
        } if model.selection.selected == Some(thread) => {
            accept_reply(model, serial, result, &mut effects);
        }
        GitMsg::Reply { .. } | GitMsg::Status(..) => {}
    }
    effects
}
