//! Pure unified triage shelf derivation, project filter option listing, and
//! thread-switch positioning.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use viz_client::protocol::{
    Attention, Project, Shelf, Standing, ThreadId, ThreadSummary, WorktreeTarget,
};

use super::{
    Model,
    cache::EnvState,
    palette::matches_filter,
    sidebar::{ProjectFilter, SHELF_WORKING, effective_shelf},
    timeline::first_non_empty_line,
};

/// Preview card metadata for the active unsent draft thread at top of sidebar.
#[derive(Debug, Clone)]
pub struct DraftPreview<'a> {
    /// Local draft sequence id.
    pub id: u64,
    /// Assigned project, if any.
    pub project: Option<&'a Project>,
    /// First non-empty line of the unsent prompt, or `"New thread"`.
    pub title: String,
    /// Selected worktree branch hint, if configured.
    pub branch: Option<String>,
    /// Selected harness id, if resolved.
    pub harness_id: Option<&'a str>,
    /// Selected harness display name, if resolved.
    pub harness_name: Option<&'a str>,
}

/// One selectable option in the Project Filter Combobox.
#[derive(Debug, Clone, Copy)]
pub enum ProjectFilterOption<'a> {
    /// Clear filter and show threads from all projects.
    All {
        /// Total non-archived, non-settled thread count.
        count: usize,
    },
    /// Filter to one registered project.
    Project {
        /// Project record.
        project: &'a Project,
        /// Non-archived, non-settled thread count in this project.
        count: usize,
    },
    /// Filter to projectless threads.
    NoProject {
        /// Non-archived, non-settled projectless thread count.
        count: usize,
    },
}

impl ProjectFilterOption<'_> {
    /// Stable key for listbox rows and active-descendant attributes.
    #[must_use]
    pub fn key(self) -> String {
        match self {
            Self::All { .. } => "project-filter-all".to_owned(),
            Self::Project { project, .. } => format!("project-filter-{}", project.path),
            Self::NoProject { .. } => "project-filter-none".to_owned(),
        }
    }

    /// Filter value applied when this option is chosen.
    #[must_use]
    pub fn to_filter(self) -> ProjectFilter {
        match self {
            Self::All { .. } => ProjectFilter::All,
            Self::Project { project, .. } => ProjectFilter::Project(project.path.clone()),
            Self::NoProject { .. } => ProjectFilter::NoProject,
        }
    }
}

/// Unified triage shelves derived once per frame from pure [`Model`] state.
#[derive(Debug)]
pub struct SidebarOutline<'a> {
    /// Currently selected thread id (parent or delegated child).
    pub open: Option<ThreadId>,
    /// Active unsent draft thread shown at the top of the list, if any.
    pub draft: Option<DraftPreview<'a>>,
    /// Pinned top-level threads in manual `pin` order.
    pub pinned: Vec<&'a ThreadSummary>,
    /// Running, waiting-on-children, or attention-awaiting top-level threads.
    pub working: Vec<&'a ThreadSummary>,
    /// Main unsettled triage queue in server/manual order.
    pub active: Vec<&'a ThreadSummary>,
    /// Snoozed threads ordered soonest wake time first.
    pub snoozed: Vec<&'a ThreadSummary>,
    /// Settled threads ordered most recently settled/updated first.
    pub settled: Vec<&'a ThreadSummary>,
    /// Wall-clock timestamp in Unix ms for relative ages and wake labels.
    pub now: i64,
}

impl<'a> SidebarOutline<'a> {
    /// Derives the filtered triage shelves from `model` at timestamp `now`.
    #[must_use]
    pub fn derive(model: &'a Model, now: i64) -> Self {
        Self::with_draft(model, now, &model.composer.draft.text)
    }

    pub fn with_draft(model: &'a Model, now: i64, text: &str) -> Self {
        let cache = model.cache();
        let known_projects: HashSet<_> = cache.projects().map(|p| p.path.as_str()).collect();
        let filter = &model.sidebar.project_filter;

        let mut outline = Self {
            open: model.selection.selected,
            draft: derive_draft(model, text, filter, &known_projects),
            pinned: Vec::new(),
            working: Vec::new(),
            active: Vec::new(),
            snoozed: Vec::new(),
            settled: Vec::new(),
            now,
        };

        for thread in cache.threads() {
            if outline.open != Some(thread.id)
                && let Some(parent) = thread.parent.and_then(|id| cache.summary(id))
                && !parent.lifecycle.archived
                && thread
                    .parent_environment
                    .as_ref()
                    .is_none_or(|env| Some(env) == model.connection.environment.as_ref())
                && filter.accepts(parent.project.as_deref(), &known_projects)
            {
                continue;
            }
            if !filter.accepts(thread.project.as_deref(), &known_projects) {
                continue;
            }
            match effective_shelf(thread, now) {
                Shelf::Archived => {}
                Shelf::Pinned => outline.pinned.push(thread),
                Shelf::Working => outline.working.push(thread),
                Shelf::Active => outline.active.push(thread),
                Shelf::Snoozed => outline.snoozed.push(thread),
                Shelf::Settled => outline.settled.push(thread),
            }
        }

        sort_shelves(&mut outline);
        outline
    }

    /// Whether all primary shelves above Settled are empty.
    #[must_use]
    pub const fn primary_empty(&self) -> bool {
        self.draft.is_none()
            && self.pinned.is_empty()
            && self.working.is_empty()
            && self.active.is_empty()
            && self.snoozed.is_empty()
    }
}

fn derive_draft<'a>(
    model: &'a Model,
    text: &str,
    filter: &ProjectFilter,
    known_projects: &HashSet<&str>,
) -> Option<DraftPreview<'a>> {
    let cache = model.cache();
    let new_thread = model.selection.new_thread.as_ref()?;
    if !filter.accepts(new_thread.project.as_deref(), known_projects) {
        return None;
    }
    let project = new_thread
        .project
        .as_deref()
        .and_then(|path| cache.project(path));
    let title = first_non_empty_line(text).map_or_else(
        || match model.composer.draft.attachments.len() {
            0 => "New thread".to_owned(),
            1 => "1 attachment".to_owned(),
            files => format!("{files} attachments"),
        },
        str::to_owned,
    );
    let branch = match &new_thread.worktree {
        Some(WorktreeTarget::New { branch, base }) => branch
            .clone()
            .or_else(|| base.clone())
            .or_else(|| Some("new worktree".into())),
        Some(WorktreeTarget::Existing { path }) => cache
            .checkouts
            .get(new_thread.project.as_deref().unwrap_or_default())
            .and_then(super::AsyncSlot::as_ready)
            .and_then(|c| c.worktrees.iter().find(|w| w.path == *path))
            .and_then(|w| w.branch.clone())
            .or_else(|| Some("worktree".into())),
        Some(WorktreeTarget::Local) | None => project.and_then(|p| p.branch.clone()),
    };
    let harness = new_thread.active_harness(model.harnesses());
    Some(DraftPreview {
        id: new_thread.id,
        project,
        title,
        branch,
        harness_id: harness.map(|h| h.id.as_str()),
        harness_name: harness.map(|h| h.name.as_str()),
    })
}

fn sort_shelves(outline: &mut SidebarOutline<'_>) {
    outline
        .pinned
        .sort_by_key(|t| (t.lifecycle.pin.unwrap_or(i64::MAX), Reverse(t.id)));
    outline.working.sort_by_key(|t| {
        let needs_user = !matches!(t.attention, Some(Attention::Approval | Attention::Question));
        let latest = t.last_input_at.unwrap_or(t.updated_at);
        (needs_user, Reverse(latest), Reverse(t.id))
    });
    outline.active.sort_by_key(|t| {
        let latest = t.last_input_at.unwrap_or(t.updated_at);
        let (first, second) = t
            .lifecycle
            .order
            .map_or((i64::MIN, Reverse(latest)), |order| (order, Reverse(0)));
        (first, second, Reverse(t.id))
    });
    outline
        .snoozed
        .sort_by_key(|t| (t.lifecycle.snoozed_until.unwrap_or(i64::MAX), Reverse(t.id)));
    outline.settled.sort_by_key(|t| {
        let at = match t.lifecycle.standing {
            Standing::Settled { at, .. } => at,
            Standing::Open | Standing::Kept => t.updated_at,
        };
        (Reverse(at), Reverse(t.id))
    });
}

/// Derives searchable options for the Project Filter Combobox: `"All
/// projects"`, pinned projects alphabetically, other projects alphabetically,
/// and `"No project"`.
#[must_use]
pub fn project_filter_options(model: &Model, now: i64) -> Vec<ProjectFilterOption<'_>> {
    let cache = model.cache();
    let query = model.sidebar.filter_query.trim().to_lowercase();
    let known: HashSet<_> = cache.projects().map(|p| p.path.as_str()).collect();

    let mut total = 0usize;
    let mut no_project = 0usize;
    let mut by_project: HashMap<&str, usize> = HashMap::new();
    for thread in cache.threads() {
        if thread.parent.is_some() {
            continue;
        }
        if matches!(
            effective_shelf(thread, now),
            Shelf::Archived | Shelf::Settled
        ) {
            continue;
        }
        total += 1;
        match thread.project.as_deref().filter(|p| known.contains(p)) {
            Some(path) => *by_project.entry(path).or_default() += 1,
            None => no_project += 1,
        }
    }

    let mut options = Vec::new();
    if matches_filter("All projects", &query) {
        options.push(ProjectFilterOption::All { count: total });
    }
    let mut projects: Vec<&Project> = cache
        .projects()
        .filter(|p| !p.settings.archived)
        .filter(|p| matches_filter(&p.name, &query) || matches_filter(&p.path, &query))
        .collect();
    projects.sort_by(|a, b| {
        b.settings
            .pinned
            .cmp(&a.settings.pinned)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    for project in projects {
        let count = by_project.get(project.path.as_str()).copied().unwrap_or(0);
        options.push(ProjectFilterOption::Project { project, count });
    }
    if matches_filter("No project", &query) {
        options.push(ProjectFilterOption::NoProject { count: no_project });
    }
    options
}

/// The topmost parent available in this environment.
#[must_use]
pub fn top_level(cache: &EnvState, id: ThreadId, environment: Option<&str>) -> ThreadId {
    let mut top = id;
    while let Some(parent) = cache
        .summary(top)
        .and_then(|thread| {
            thread.parent.filter(|_| {
                thread
                    .parent_environment
                    .as_deref()
                    .is_none_or(|parent| Some(parent) == environment)
            })
        })
        .filter(|parent| *parent < top && cache.summary(*parent).is_some())
    {
        top = parent;
    }
    top
}

/// Where `id`'s top-level thread sits in the unified sidebar triage order,
/// counting from the top, for spatial thread-switch motion.
#[must_use]
pub fn sidebar_position(model: &Model, id: ThreadId) -> Option<usize> {
    let top = top_level(model.cache(), id, model.connection.environment.as_deref());
    let outline = SidebarOutline::derive(model, 0);
    outline
        .pinned
        .iter()
        .chain(&outline.working)
        .chain(&outline.active)
        .chain(&outline.snoozed)
        .chain(&outline.settled)
        .position(|thread| thread.id == top)
}

/// Returns the `index`-th visible primary thread (`0`-based) for
/// `Cmd/Ctrl+1..9` quick-jump shortcuts.
#[must_use]
pub fn nth_visible_thread(model: &Model, index: usize, now: i64) -> Option<ThreadId> {
    let outline = SidebarOutline::derive(model, now);
    let working_open = model.sidebar.is_shelf_open(SHELF_WORKING);
    let working_iter = if working_open {
        outline.working.as_slice()
    } else {
        &[]
    };
    outline
        .pinned
        .iter()
        .chain(working_iter)
        .chain(&outline.active)
        .nth(index)
        .map(|thread| thread.id)
}
