//! Normalised per-environment server state cache.

use std::collections::BTreeMap;

use viz_client::protocol::{
    GitBranch, Harness, Project, Task, ThreadId, ThreadState, ThreadSummary, Worktree,
};

use super::transcript::Transcript;

/// A Git project's branches and worktrees loaded on demand for checkout menus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkouts {
    /// Local and remote-tracking branches.
    pub branches: Vec<GitBranch>,
    /// Main and linked checkouts of the repository.
    pub worktrees: Vec<Worktree>,
}

/// Cached transcript and live state for an open/watched thread.
#[derive(Debug, Clone, Default)]
pub struct OpenThread {
    /// Committed transcript items and live uncommitted ACP tail.
    pub transcript: Transcript,
    /// Latest watched thread state, once received from the server.
    pub state: Option<ThreadState>,
    /// True after selecting the thread until its initial view arrives.
    pub loading: bool,
    /// True while a page of older entries is being fetched.
    pub history_loading: bool,
}

/// Cached project icon fetch metadata keyed by project path.
///
/// Decoded GPU textures live outside the pure model in `ShellWidgets`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectIconCache {
    /// Server-supplied `ETag` of the cached image bytes, if any.
    pub etag: Option<String>,
    /// Whether the project has an image representation decoded in shell state.
    pub has_image: bool,
    /// True once a `project.icon` request has been issued for the current sync.
    pub requested: bool,
}

/// Normalised cache of server-owned data for one environment.
///
/// Upcoming Phase 1 tracks extend this cache with git working-tree status,
/// diffs, pull requests, pending approval/question requests, and docked
/// terminal state as those protocol types land.
#[derive(Debug, Clone, Default)]
pub struct EnvironmentCache {
    projects: BTreeMap<String, Project>,
    project_order: Vec<String>,
    threads: BTreeMap<ThreadId, ThreadSummary>,
    thread_order: Vec<ThreadId>,
    /// Cached transcripts and live states keyed by thread id.
    pub open_threads: BTreeMap<ThreadId, OpenThread>,
    /// Branches and worktrees keyed by project path.
    pub checkouts: BTreeMap<String, Checkouts>,
    /// Fetched project icon metadata keyed by project path.
    pub project_icons: BTreeMap<String, ProjectIconCache>,
    /// Configured ACP harnesses advertised by this environment.
    pub harnesses: Vec<Harness>,
    /// Delegated tasks tracked in this environment.
    pub tasks: Vec<Task>,
}

impl EnvironmentCache {
    /// Clears cached server data when resetting or switching environments.
    pub fn clear(&mut self) {
        self.projects.clear();
        self.project_order.clear();
        self.threads.clear();
        self.thread_order.clear();
        self.open_threads.clear();
        self.checkouts.clear();
        self.project_icons.clear();
        self.harnesses.clear();
        self.tasks.clear();
    }

    /// Replaces the cached projects while preserving caller order.
    pub fn set_projects(&mut self, projects: Vec<Project>) {
        self.projects.clear();
        self.project_order.clear();
        self.project_order.reserve(projects.len());
        for project in projects {
            let path = project.path.clone();
            if self.projects.insert(path.clone(), project).is_none() {
                self.project_order.push(path);
            }
        }
    }

    /// Inserts a newly created project in sorted `(name, path)` order if not
    /// already present.
    pub fn insert_project_sorted(&mut self, project: Project) {
        if self.projects.contains_key(&project.path) {
            return;
        }
        let path = project.path.clone();
        let at = self.project_order.partition_point(|known_path| {
            self.projects
                .get(known_path)
                .is_some_and(|known| (&known.name, &known.path) < (&project.name, &project.path))
        });
        self.projects.insert(path.clone(), project);
        self.project_order.insert(at, path);
    }

    /// Appends a project at the end of the display order (for scene fixtures).
    pub fn push_project(&mut self, project: Project) {
        let path = project.path.clone();
        if self.projects.insert(path.clone(), project).is_none() {
            self.project_order.push(path);
        }
    }

    /// Removes a project and returns its prior position and value for rollback.
    pub fn remove_project(&mut self, path: &str) -> Option<(usize, Project)> {
        let project = self.projects.remove(path)?;
        let index = self
            .project_order
            .iter()
            .position(|known| known == path)
            .unwrap_or_default();
        if index < self.project_order.len() && self.project_order[index] == path {
            self.project_order.remove(index);
        }
        Some((index, project))
    }

    /// Restores a previously removed project at `index`.
    pub fn restore_project(&mut self, index: usize, project: Project) {
        let path = project.path.clone();
        if self.projects.insert(path.clone(), project).is_none() {
            let at = index.min(self.project_order.len());
            self.project_order.insert(at, path);
        }
    }

    /// Looks up a project by its absolute path.
    #[must_use]
    pub fn project(&self, path: &str) -> Option<&Project> {
        self.projects.get(path)
    }

    /// Mutable lookup for a project by path (used by optimistic updates).
    pub fn project_mut(&mut self, path: &str) -> Option<&mut Project> {
        self.projects.get_mut(path)
    }

    /// Projects in display order.
    pub fn projects(&self) -> impl Iterator<Item = &Project> {
        self.project_order
            .iter()
            .filter_map(|path| self.projects.get(path))
    }

    /// Whether any projects are cached.
    #[must_use]
    pub const fn has_projects(&self) -> bool {
        !self.projects.is_empty()
    }

    /// Removes all cached projects.
    pub fn clear_projects(&mut self) {
        self.projects.clear();
        self.project_order.clear();
    }

    /// Replaces the cached thread summaries while preserving caller order.
    pub fn set_threads(&mut self, threads: Vec<ThreadSummary>) {
        self.threads.clear();
        self.thread_order.clear();
        self.thread_order.reserve(threads.len());
        for thread in threads {
            let id = thread.id;
            if self.threads.insert(id, thread).is_none() {
                self.thread_order.push(id);
            }
        }
    }

    /// Retains only thread summaries matching `predicate`.
    pub fn retain_threads(&mut self, mut predicate: impl FnMut(&ThreadSummary) -> bool) {
        self.threads.retain(|_, thread| predicate(thread));
        self.thread_order.retain(|id| self.threads.contains_key(id));
    }

    /// Clears all cached thread summaries.
    pub fn clear_threads(&mut self) {
        self.threads.clear();
        self.thread_order.clear();
    }

    /// Looks up a thread summary by id.
    #[must_use]
    pub fn summary(&self, id: ThreadId) -> Option<&ThreadSummary> {
        self.threads.get(&id)
    }

    /// Mutable lookup for a thread summary by id.
    pub fn summary_mut(&mut self, id: ThreadId) -> Option<&mut ThreadSummary> {
        self.threads.get_mut(&id)
    }

    /// Thread summaries in display order.
    pub fn threads(&self) -> impl Iterator<Item = &ThreadSummary> {
        self.thread_order
            .iter()
            .filter_map(|id| self.threads.get(id))
    }

    /// Number of cached threads.
    #[must_use]
    pub const fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Returns the open-thread cache entry for `id`, if loaded.
    #[must_use]
    pub fn open_thread(&self, id: ThreadId) -> Option<&OpenThread> {
        self.open_threads.get(&id)
    }

    /// Returns or creates the open-thread cache entry for `id`.
    pub fn open_thread_mut(&mut self, id: ThreadId) -> &mut OpenThread {
        self.open_threads.entry(id).or_default()
    }
}
