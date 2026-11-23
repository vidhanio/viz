//! Pure sidebar derivations: filtering, parent/child task trees, ordering,
//! and thread switch positioning.

use std::collections::{HashMap, HashSet};

use viz_client::protocol::{Project, ThreadId, ThreadStatus, ThreadSummary};

use super::{Model, SidebarMode, cache::EnvironmentCache, thread_title};

/// Top-level threads and their delegated child trees, filtered once per frame.
#[derive(Debug)]
pub struct SidebarOutline<'a> {
    /// Exact open thread (parent or delegated child).
    pub open: Option<ThreadId>,
    /// Non-pinned threads grouped by known project path.
    pub projects: HashMap<&'a str, Vec<&'a ThreadSummary>>,
    /// Non-pinned threads without a known project.
    pub loose: Vec<&'a ThreadSummary>,
    /// Pinned top-level threads, newest first.
    pub pinned: Vec<&'a ThreadSummary>,
    /// All non-pinned top-level threads, newest first.
    pub recent: Vec<&'a ThreadSummary>,
    /// Local delegated child threads indexed by their parent thread id.
    pub children: HashMap<ThreadId, Vec<&'a ThreadSummary>>,
    /// Normalized lowercase filter query.
    pub query: String,
    /// Wall-clock timestamp in Unix ms for relative ages.
    pub now: i64,
}

impl<'a> SidebarOutline<'a> {
    /// Derives the filtered sidebar outline from `model` at timestamp `now`.
    #[must_use]
    pub fn derive(model: &'a Model, now: i64) -> Self {
        let cache = model.cache();
        let query = model.sidebar.filter.trim().to_lowercase();
        let mut outline = Self {
            open: model.selection.selected,
            projects: HashMap::new(),
            loose: Vec::new(),
            pinned: Vec::new(),
            recent: Vec::new(),
            children: HashMap::new(),
            query,
            now,
        };
        let known_projects: HashSet<_> = cache.projects().map(|p| p.path.as_str()).collect();
        let matching_projects: HashSet<_> = cache
            .projects()
            .filter(|project| outline.matches(&project.name) || outline.matches(&project.path))
            .map(|project| project.path.as_str())
            .collect();
        let mut child_ids = HashSet::new();
        for thread in cache.threads() {
            if thread.lifecycle.archived {
                continue;
            }
            if let Some(parent) = thread.parent.filter(|&parent| {
                thread
                    .parent_environment
                    .as_ref()
                    .is_none_or(|env| Some(env) == model.connection.environment.as_ref())
                    && parent < thread.id
                    && cache.summary(parent).is_some()
            }) {
                child_ids.insert(thread.id);
                outline.children.entry(parent).or_default().push(thread);
            }
        }
        for kids in outline.children.values_mut() {
            kids.sort_by_key(|thread| {
                (
                    thread.status != ThreadStatus::Working,
                    std::cmp::Reverse(thread.updated_at),
                )
            });
        }
        for thread in cache.threads() {
            if child_ids.contains(&thread.id) || thread.lifecycle.archived {
                continue;
            }
            let project = thread
                .project
                .as_deref()
                .filter(|id| known_projects.contains(id));
            if !outline.matches(thread_title(thread))
                && !thread
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.branch.as_deref())
                    .is_some_and(|branch| outline.matches(branch))
                && !project.is_some_and(|id| matching_projects.contains(id))
                && !outline.descendant_matches(thread.id)
            {
                continue;
            }
            if thread.lifecycle.pin.is_some() {
                outline.pinned.push(thread);
            } else {
                outline.recent.push(thread);
                if let Some(project) = project {
                    outline.projects.entry(project).or_default().push(thread);
                } else {
                    outline.loose.push(thread);
                }
            }
        }
        let newest = |threads: &mut Vec<&ThreadSummary>| {
            threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
        };
        newest(&mut outline.pinned);
        newest(&mut outline.recent);
        newest(&mut outline.loose);
        outline.projects.values_mut().for_each(newest);
        outline
    }

    /// Whether `value` matches the active sidebar filter.
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        self.query.is_empty() || value.to_lowercase().contains(&self.query)
    }

    fn descendant_matches(&self, parent: ThreadId) -> bool {
        let Some(kids) = self.children.get(&parent) else {
            return false;
        };
        kids.iter()
            .any(|child| self.matches(thread_title(child)) || self.descendant_matches(child.id))
    }

    /// Whether any descendant of `parent` is currently working or selected.
    #[must_use]
    pub fn has_active_or_selected_child(&self, parent: ThreadId) -> bool {
        let Some(kids) = self.children.get(&parent) else {
            return false;
        };
        kids.iter().any(|child| {
            child.status == ThreadStatus::Working
                || self.open == Some(child.id)
                || self.has_active_or_selected_child(child.id)
        })
    }

    /// Whether `project` should be shown in Projects mode under the current
    /// filter.
    #[must_use]
    pub fn visible_project(&self, project: &Project) -> bool {
        self.projects.contains_key(project.path.as_str())
            || self.matches(&project.name)
            || self.matches(&project.path)
    }
}

/// The topmost parent available in this environment.
#[must_use]
pub fn top_level(cache: &EnvironmentCache, id: ThreadId, environment: Option<&str>) -> ThreadId {
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

/// Where `id`'s top-level thread is listed in the sidebar, counting from the
/// top, for determining thread-switch travel direction.
#[must_use]
pub fn sidebar_position(model: &Model, id: ThreadId) -> Option<usize> {
    let top = top_level(model.cache(), id, model.connection.environment.as_deref());
    let outline = SidebarOutline::derive(model, 0);
    let listed: Vec<&ThreadSummary> = match model.sidebar.mode {
        SidebarMode::Recent => outline.recent.clone(),
        SidebarMode::Projects => model
            .cache()
            .projects()
            .filter_map(|project| outline.projects.get(project.path.as_str()))
            .flatten()
            .chain(&outline.loose)
            .copied()
            .collect(),
    };
    outline
        .pinned
        .iter()
        .chain(&listed)
        .position(|thread| thread.id == top)
}

/// First visible thread matching the active sidebar filter (for Enter submit).
#[must_use]
pub fn sidebar_first_match(model: &Model) -> Option<ThreadId> {
    let outline = SidebarOutline::derive(model, 0);
    if outline.query.is_empty() {
        return None;
    }
    match model.sidebar.mode {
        SidebarMode::Recent => outline
            .pinned
            .iter()
            .chain(&outline.recent)
            .next()
            .map(|thread| thread.id),
        SidebarMode::Projects => outline
            .pinned
            .iter()
            .chain(model.cache().projects().filter_map(|project| {
                outline
                    .projects
                    .get(project.path.as_str())
                    .and_then(|threads| threads.first())
            }))
            .chain(&outline.loose)
            .next()
            .map(|thread| thread.id),
    }
}
