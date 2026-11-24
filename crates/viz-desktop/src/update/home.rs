//! Pure transitions for the Home hero Project Picker, bottom Context Strip
//! branch/worktree pickers, project pin toggles, and Access Mode selector.

use super::{NavKey, selection::focus_draft};
use crate::{
    effects::{EditSync, EditTarget, Effect, ProjectChange, ServerEffect},
    model::{Model, OptimisticUndo, draft_branch_options, home_project_options},
};

/// Element key of the Home hero Project Picker search input.
pub const HOME_PROJECT_SEARCH: &str = "home-project-search";
/// Element key of the Home context strip branch picker search input.
pub const DRAFT_BRANCH_SEARCH: &str = "draft-branch-search";
/// `open_picker` key for the Home hero Project Picker.
pub const HOME_PROJECT_PICKER: &str = "draft-project";
/// `open_picker` key for the Home context strip branch/base-ref picker.
pub const DRAFT_BRANCH_PICKER: &str = "draft-branch";
/// `open_picker` key for the Home environment selector popover.
pub const HOME_ENV_PICKER: &str = "home-env";

pub(super) fn toggle_home_project_picker(model: &mut Model, effects: &mut Vec<Effect>) {
    let blocked = model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|thread| thread.sending);
    if model.composer.open_picker.as_deref() == Some(HOME_PROJECT_PICKER) || blocked {
        close_home_project_picker(model, effects);
    } else {
        model.composer.open_picker = Some(HOME_PROJECT_PICKER.to_owned());
        model.composer.home_project.query.clear();
        model.composer.home_project.highlighted = 0;
        effects.push(Effect::SyncEdit(EditSync::Clear(
            EditTarget::HomeProjectQuery,
        )));
        effects.push(Effect::Focus {
            key: HOME_PROJECT_SEARCH,
            select_all: false,
            when_shown: true,
        });
    }
}

pub(super) fn close_home_project_picker(model: &mut Model, effects: &mut Vec<Effect>) {
    if model.composer.open_picker.as_deref() == Some(HOME_PROJECT_PICKER) {
        model.composer.open_picker = None;
    }
    model.composer.home_project.query.clear();
    model.composer.home_project.highlighted = 0;
    effects.push(Effect::SyncEdit(EditSync::Clear(
        EditTarget::HomeProjectQuery,
    )));
}

pub(super) fn handle_home_project_key(model: &mut Model, key: NavKey, effects: &mut Vec<Effect>) {
    let options = home_project_options(model);
    match key {
        NavKey::Escape => {
            close_home_project_picker(model, effects);
            effects.push(focus_draft());
        }
        NavKey::Down if !options.is_empty() => {
            model.composer.home_project.highlighted =
                (model.composer.home_project.highlighted + 1) % options.len();
        }
        NavKey::Up if !options.is_empty() => {
            model.composer.home_project.highlighted = model
                .composer
                .home_project
                .highlighted
                .checked_sub(1)
                .unwrap_or(options.len() - 1);
        }
        NavKey::Enter => {
            let idx = model
                .composer
                .home_project
                .highlighted
                .min(options.len().saturating_sub(1));
            if let Some(option) = options.get(idx) {
                let path = option.path();
                effects.extend(super::selection::update(
                    model,
                    super::SelectionMsg::ChooseDraftProject(path),
                ));
            }
        }
        NavKey::Up | NavKey::Down | NavKey::Tab => {}
    }
}

pub(super) fn toggle_project_pin(model: &mut Model, path: String, effects: &mut Vec<Effect>) {
    if !model.connected() {
        return;
    }
    let env = model.active_env_key().to_owned();
    let Some(prior_pinned) = model.cache_mut().project_mut(&path).map(|project| {
        let prior = project.settings.pinned;
        project.settings.pinned = !prior;
        prior
    }) else {
        return;
    };
    let base_rev = model.cache().rev;
    let op_id = model.optimistic.record(
        base_rev,
        OptimisticUndo::ProjectPin {
            env,
            path: path.clone(),
            prior_pinned,
        },
    );
    effects.push(Effect::Server(ServerEffect::Project {
        op_id: Some(op_id),
        change: ProjectChange::Pin {
            id: path,
            pinned: !prior_pinned,
        },
    }));
}

pub(super) fn toggle_draft_branch_picker(
    model: &mut Model,
    project: String,
    effects: &mut Vec<Effect>,
) {
    let blocked = model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|draft| draft.sending);
    if model.composer.open_picker.as_deref() == Some(DRAFT_BRANCH_PICKER) || blocked {
        model.composer.open_picker = None;
        model.composer.draft_branch.query.clear();
        model.composer.draft_branch.highlighted = 0;
        effects.push(Effect::SyncEdit(EditSync::Clear(
            EditTarget::DraftBranchQuery,
        )));
    } else {
        model.composer.open_picker = Some(DRAFT_BRANCH_PICKER.to_owned());
        model.composer.draft_branch.query.clear();
        model.composer.draft_branch.highlighted = 0;
        effects.push(Effect::SyncEdit(EditSync::Clear(
            EditTarget::DraftBranchQuery,
        )));
        effects.push(Effect::Server(ServerEffect::LoadCheckouts(project)));
        effects.push(Effect::Focus {
            key: DRAFT_BRANCH_SEARCH,
            select_all: false,
            when_shown: true,
        });
    }
}

pub(super) fn handle_draft_branch_key(model: &mut Model, key: NavKey, effects: &mut Vec<Effect>) {
    let Some(project) = model.project().cloned() else {
        return;
    };
    let branches = draft_branch_options(model, &project);
    match key {
        NavKey::Escape => {
            model.composer.open_picker = None;
            model.composer.draft_branch.query.clear();
            model.composer.draft_branch.highlighted = 0;
            effects.push(Effect::SyncEdit(EditSync::Clear(
                EditTarget::DraftBranchQuery,
            )));
            effects.push(focus_draft());
        }
        NavKey::Down | NavKey::Up if !branches.is_empty() => {
            let count = branches.len();
            let current = model.composer.draft_branch.highlighted.min(count - 1);
            let next = if key == NavKey::Up {
                (current + count - 1) % count
            } else {
                (current + 1) % count
            };
            let item = format!("draft-branch-{}", branches[next].name);
            model.composer.draft_branch.highlighted = next;
            effects.push(Effect::ScrollIntoView {
                list: "draft-branch-combobox-scroll",
                item,
            });
        }
        NavKey::Enter => {
            let idx = model
                .composer
                .draft_branch
                .highlighted
                .min(branches.len().saturating_sub(1));
            if let Some(branch) = branches.get(idx) {
                let name = branch.name.clone();
                let upstream = branch.upstream.clone();
                effects.extend(super::selection::update(
                    model,
                    super::SelectionMsg::ChooseDraftBranch { name, upstream },
                ));
            }
        }
        NavKey::Up | NavKey::Down | NavKey::Tab => {}
    }
}
