//! Pure derivations for the Home hero Project Picker combobox and bottom
//! Context Strip branch/worktree selectors.

use std::cmp::Reverse;

use viz_client::protocol::{GitBranch, Project, WorktreeTarget};

use super::{Model, palette::matches_filter};

/// One selectable option inside the Home hero Project Picker combobox.
#[derive(Debug, Clone, Copy)]
pub enum HomeProjectOption<'a> {
    /// A registered workspace project.
    Project(&'a Project),
    /// Clear the project assignment (`No project`).
    NoProject,
}

impl HomeProjectOption<'_> {
    /// Stable element key for listbox rows and active-descendant wiring.
    #[must_use]
    pub fn key(self) -> String {
        match self {
            Self::Project(project) => format!("home-project-{}", project.path),
            Self::NoProject => "home-project-none".to_owned(),
        }
    }

    /// Project path assigned when selected (`None` for `"No project"`).
    #[must_use]
    pub fn path(self) -> Option<String> {
        match self {
            Self::Project(project) => Some(project.path.clone()),
            Self::NoProject => None,
        }
    }
}

/// Derives searchable options for the Home hero Project Picker: pinned
/// projects first, then remaining projects ordered by most recent thread
/// activity and name, plus `"No project"` when switching away from a project
/// or matching the query.
#[must_use]
pub fn home_project_options(model: &Model) -> Vec<HomeProjectOption<'_>> {
    let query = model.composer.home_project.query.trim().to_lowercase();
    let cache = model.cache();
    let mut recent_by_project = std::collections::HashMap::<&str, i64>::new();
    for thread in cache.threads() {
        if let Some(path) = thread.project.as_deref() {
            let entry = recent_by_project.entry(path).or_insert(0);
            *entry = (*entry).max(thread.updated_at);
        }
    }
    let mut projects: Vec<&Project> = cache
        .projects()
        .filter(|project| !project.settings.archived)
        .filter(|project| {
            query.is_empty()
                || matches_filter(&project.name, &query)
                || matches_filter(&project.path, &query)
        })
        .collect();
    projects.sort_by(|a, b| {
        let a_recent = recent_by_project.get(a.path.as_str()).copied().unwrap_or(0);
        let b_recent = recent_by_project.get(b.path.as_str()).copied().unwrap_or(0);
        b.settings
            .pinned
            .cmp(&a.settings.pinned)
            .then_with(|| Reverse(a_recent).cmp(&Reverse(b_recent)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut out: Vec<HomeProjectOption<'_>> = projects
        .into_iter()
        .map(HomeProjectOption::Project)
        .collect();
    let has_project = model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|draft| draft.project.is_some());
    if (has_project && query.is_empty())
        || (!query.is_empty() && matches_filter("No project", &query))
    {
        out.push(HomeProjectOption::NoProject);
    }
    out
}

/// Filtered branches for the Home Context Strip branch/base-ref picker.
#[must_use]
pub fn draft_branch_options<'a>(model: &'a Model, project: &'a Project) -> Vec<&'a GitBranch> {
    let query = model.composer.draft_branch.query.trim().to_lowercase();
    let Some(checkouts) = model.checkouts(&project.path) else {
        return Vec::new();
    };
    let is_new_worktree = model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|draft| matches!(draft.worktree, Some(WorktreeTarget::New { .. })));
    let remote_base = model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|draft| draft.remote_base);
    checkouts
        .branches
        .iter()
        .filter(|branch| {
            if is_new_worktree && remote_base && !branch.remote && branch.upstream.is_none() {
                return true;
            }
            query.is_empty() || matches_filter(&branch.name, &query)
        })
        .filter(|branch| query.is_empty() || matches_filter(&branch.name, &query))
        .collect()
}
