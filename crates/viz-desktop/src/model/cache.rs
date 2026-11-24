//! Per-environment state: normalised server workspace data plus client-owned
//! per-environment drafts, folds, and prompt history.

use std::collections::{BTreeMap, HashMap};

use viz_client::protocol::{
    EntryAnchor, EntryId, GitBranch, Harness, Project, Task, ThreadId, ThreadState, ThreadSummary,
    Worktree,
};

use super::{async_slot::AsyncSlot, composer_state::DraftData, transcript::Transcript};

/// A Git project's branches and worktrees loaded on demand for checkout menus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkouts {
    /// Local and remote-tracking branches.
    pub branches: Vec<GitBranch>,
    /// Main and linked checkouts of the repository.
    pub worktrees: Vec<Worktree>,
}

/// Active transcript, server state, and inline media fetch status for an open
/// thread.
#[derive(Debug, Clone, Default)]
pub struct OpenThread {
    /// Committed transcript window and live uncommitted ACP tail.
    pub transcript: Transcript,
    /// Latest watched thread state, once received from the server.
    pub state: Option<ThreadState>,
    /// True after selecting the thread until its initial view arrives.
    pub loading: bool,
    /// Detached search window while the watched transcript continues updating.
    pub history: Option<HistoryWindow>,
    /// Anchor of the outstanding viewport request.
    pub history_request: Option<EntryAnchor>,
    /// Per-attachment inline preview fetch lifecycle, keyed by attachment id.
    ///
    /// Decoded GPU textures live outside the pure model in `ShellWidgets`,
    /// strictly scoped to the active open thread.
    pub media_slots: HashMap<String, AsyncSlot<()>>,
}

/// Search-centered transcript and its exclusive forward cursor.
#[derive(Debug, Clone)]
pub struct HistoryWindow {
    pub transcript: Transcript,
    pub after: Option<EntryId>,
    /// Last layout report for the detached scroll viewport.
    pub at_bottom: Option<bool>,
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

/// Consolidated server-synchronized and client-local state for one environment.
#[derive(Debug, Clone, Default)]
pub struct EnvState {
    /// Latest applied server workspace revision (`0` before first snapshot).
    pub rev: u64,
    projects: BTreeMap<String, Project>,
    project_order: Vec<String>,
    threads: BTreeMap<ThreadId, ThreadSummary>,
    thread_order: Vec<ThreadId>,
    /// Open transcripts and live states keyed by thread id.
    pub open_threads: BTreeMap<ThreadId, OpenThread>,
    /// Branches and worktrees keyed by project path.
    pub checkouts: BTreeMap<String, AsyncSlot<Checkouts>>,
    /// Fetched project icon metadata keyed by project path.
    pub project_icons: BTreeMap<String, ProjectIconCache>,
    /// Configured ACP harnesses advertised by this environment.
    pub harnesses: Vec<Harness>,
    /// Delegated tasks tracked in this environment.
    pub tasks: Vec<Task>,
    /// Saved unsent composer drafts keyed by thread id.
    pub drafts: BTreeMap<ThreadId, DraftData>,
    /// Per-thread disclosure overrides (`true` = open, `false` = closed).
    pub folds: BTreeMap<ThreadId, HashMap<String, bool>>,
    /// Sent prompt recall history keyed by thread id.
    pub prompt_history: BTreeMap<ThreadId, Vec<viz_client::protocol::Prompt>>,
}

impl EnvState {
    /// Clears server-synchronized data when resetting or switching environments
    /// while preserving client-owned drafts, fold overrides, and prompt
    /// history.
    pub fn clear_server_data(&mut self) {
        self.rev = 0;
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

    /// Inserts or replaces a project and keeps display order sorted by
    /// `(name, path)`.
    pub fn upsert_project(&mut self, project: Project) {
        let path = project.path.clone();
        let inserted = self.projects.insert(path.clone(), project).is_none();
        if inserted {
            self.project_order.push(path);
        }
        self.project_order.sort_unstable_by(|a, b| {
            let pa = self.projects.get(a);
            let pb = self.projects.get(b);
            match (pa, pb) {
                (Some(pa), Some(pb)) => pa.name.cmp(&pb.name).then_with(|| pa.path.cmp(&pb.path)),
                _ => a.cmp(b),
            }
        });
    }

    /// Inserts or updates a single thread summary in place without rebuilding
    /// the whole workspace snapshot.
    pub fn upsert_thread(&mut self, thread: ThreadSummary) {
        let id = thread.id;
        if self.threads.insert(id, thread).is_none() {
            self.thread_order.insert(0, id);
        }
    }

    /// Removes one thread summary and any cached open transcript for it.
    pub fn remove_thread(&mut self, id: ThreadId) -> Option<ThreadSummary> {
        let removed = self.threads.remove(&id)?;
        self.thread_order.retain(|known| *known != id);
        self.open_threads.remove(&id);
        Some(removed)
    }

    /// Inserts or updates a single delegated task (newest first on insert).
    pub fn upsert_task(&mut self, task: Task) {
        if let Some(existing) = self.tasks.iter_mut().find(|item| item.id == task.id) {
            *existing = task;
        } else {
            self.tasks.insert(0, task);
        }
    }

    /// Removes a delegated task by id.
    pub fn remove_task(&mut self, id: &str) {
        self.tasks.retain(|task| task.id != id);
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

    /// Drops superseded watches once a new transcript snapshot arrives.
    pub fn retain_open_thread(&mut self, id: ThreadId) {
        self.open_threads.retain(|thread, _| *thread == id);
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
