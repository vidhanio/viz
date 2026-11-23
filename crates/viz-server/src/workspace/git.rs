//! Resolve Git requests against thread checkouts and coordinate mutations with
//! runs.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use viz_git::{CheckoutActivity, DiffTarget, Head, Repository};
use viz_protocol::{
    GitAction, GitActionRequest, GitActionResult, GitDiff, GitDiffFile, GitDiffLine,
    GitDiffRequest, GitDiffTarget, GitFileKind, GitHunk, GitLineStats, GitStatus, ThreadId,
};

use super::{Workspace, store};
use crate::error::Error;

impl Workspace {
    pub(crate) async fn git_repository(&self, thread: ThreadId) -> Result<Repository, Error> {
        let binding = store::binding(&mut *self.db.lock().await, thread).await?;
        Ok(Repository::discover(Path::new(&binding.checkout.cwd), &self.closing).await?)
    }

    pub(crate) async fn git_status(&self, repo: &Repository) -> Result<GitStatus, Error> {
        let checkout = checkout_path(repo)?;
        let cancel = &self.closing;
        let (head, entries, tracking, diff, operation) = tokio::try_join!(
            repo.head(cancel),
            repo.status(cancel),
            repo.ahead_behind(cancel),
            repo.diff(DiffTarget::Uncommitted, cancel),
            repo.operation(cancel),
        )?;
        let (branch, commit) = match head {
            Head::Branch { name, commit } => (Some(name), Some(commit)),
            Head::Detached { commit } => (None, Some(commit)),
            Head::Unborn { name } => (Some(name), None),
        };
        let (upstream, ahead, behind) = tracking.map_or((None, 0, 0), |tracking| {
            (Some(tracking.upstream), tracking.ahead, tracking.behind)
        });
        let operation = self
            .reservations
            .lock()
            .actions
            .get(checkout)
            .map(|action| action.label().to_owned())
            .or(operation);
        Ok(GitStatus {
            checkout: store::path_text(checkout)?.to_owned(),
            branch,
            commit,
            upstream,
            ahead,
            behind,
            staged: entries.iter().filter(|entry| entry.staged).count(),
            unstaged: entries.iter().filter(|entry| entry.unstaged).count(),
            untracked: entries.iter().filter(|entry| entry.untracked).count(),
            conflicted: entries.iter().filter(|entry| entry.conflicted).count(),
            stats: line_stats(diff.stats),
            operation,
        })
    }

    pub(super) async fn git_action(
        self: &Arc<Self>,
        request: GitActionRequest,
    ) -> Result<GitActionResult, Error> {
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move { workspace.apply_git_action(request).await })
            .await
            .map_err(viz_git::Error::from)?
    }

    /// Reserves the thread's checkout, then runs the action with the database
    /// free. Actions that move `HEAD` or rewrite the working tree also exclude
    /// turns in that checkout; see [`Reservations`].
    async fn apply_git_action(&self, request: GitActionRequest) -> Result<GitActionResult, Error> {
        let GitActionRequest { thread, action } = request;
        let repo = self.git_repository(thread).await?;
        let checkout = checkout_path(&repo)?.to_owned();
        let exclusive = matches!(action, GitAction::SwitchBranch { .. } | GitAction::Pull);
        let reservation = self.reserve(
            &checkout,
            CheckoutAction::Git {
                label: action_name(&action),
                exclusive,
            },
        )?;
        let result = run_action(&repo, action, &self.closing).await;
        drop(reservation);
        if exclusive && result.is_ok() {
            self.invalidate_checkout_pulls(&checkout);
        }
        result.map(|commit| GitActionResult { commit })
    }

    /// Claims `root` for `action`, failing with [`viz_git::Error::Busy`] while
    /// another action holds it or, for an exclusive action, while a turn runs
    /// in it. The claim lasts until the returned guard drops.
    pub(super) fn reserve(
        &self,
        root: &Path,
        action: CheckoutAction,
    ) -> Result<Reservation<'_>, Error> {
        let mut held = self.reservations.lock();
        let covers = |other: &Path| match action {
            CheckoutAction::RemoveWorktree => other.starts_with(root),
            CheckoutAction::Git { .. } => other == root,
        };
        let busy = held.actions.contains_key(root)
            || held.actions.iter().any(|(other, held)| {
                matches!(held, CheckoutAction::RemoveWorktree) && root.starts_with(other)
            })
            || (action.excludes_turns() && held.turns.keys().any(|turn| covers(turn)));
        if busy {
            return Err(viz_git::Error::Busy.into());
        }
        let exclusive = action.excludes_turns();
        held.actions.insert(root.to_owned(), action);
        drop(held);
        self.git_activity();
        Ok(Reservation {
            workspace: self,
            root: root.to_owned(),
            exclusive,
        })
    }

    /// Registers a turn in `root`, or returns `None` while an exclusive action
    /// holds it. A turn outside any repository always proceeds.
    pub(super) fn occupy(&self, root: Option<&Path>) -> Option<Occupancy<'_>> {
        let Some(root) = root else {
            return Some(Occupancy {
                workspace: self,
                root: None,
            });
        };
        let mut held = self.reservations.lock();
        if held.excludes(root) {
            return None;
        }
        *held.turns.entry(root.to_owned()).or_default() += 1;
        drop(held);
        Some(Occupancy {
            workspace: self,
            root: Some(root.to_owned()),
        })
    }

    /// Whether `path` lies in a worktree being removed.
    pub(super) fn removing(&self, path: &Path) -> bool {
        self.reservations
            .lock()
            .actions
            .iter()
            .any(|(root, action)| {
                matches!(action, CheckoutAction::RemoveWorktree) && path.starts_with(root)
            })
    }

    /// The checkout root a thread's turns occupy, if it runs in a repository.
    pub(super) async fn turn_root(&self, thread: ThreadId) -> Result<Option<PathBuf>, Error> {
        match self.git_repository(thread).await {
            Ok(repo) => Ok(repo.identity().checkout.clone()),
            Err(Error::Git(_) | Error::NoThread(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) async fn git_diff(&self, request: GitDiffRequest) -> Result<GitDiff, Error> {
        let repo = self.git_repository(request.thread).await?;
        let target = match &request.target {
            GitDiffTarget::Working => DiffTarget::Uncommitted,
            GitDiffTarget::Staged => DiffTarget::Staged,
            GitDiffTarget::Base { revision } => DiffTarget::Base(revision),
            GitDiffTarget::Commit { revision } => DiffTarget::Commit(revision),
            GitDiffTarget::Turn { run } => {
                return self
                    .checkpoint_diff(&repo, request.thread, Some(*run))
                    .await;
            }
            GitDiffTarget::Thread => {
                return self.checkpoint_diff(&repo, request.thread, None).await;
            }
        };
        Ok(wire_diff(repo.diff(target, &self.closing).await?))
    }
}

/// Exclusive operations on checkouts and the turns running in them.
///
/// Git work runs with the database free, so this registry, not the write
/// lease, keeps a pull, branch switch, checkpoint restore, or worktree removal
/// from overlapping a turn in the same checkout. Workers register a turn
/// under the write lease as they claim it and skip the claim while an
/// exclusive action holds the checkout; releasing that action wakes them.
#[derive(Default)]
pub(super) struct Reservations(std::sync::Mutex<Held>);

#[derive(Default)]
pub(super) struct Held {
    actions: HashMap<PathBuf, CheckoutAction>,
    /// Turns under way per checkout root.
    turns: HashMap<PathBuf, usize>,
}

impl Reservations {
    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.0.lock().expect("checkout reservations poisoned")
    }
}

impl Held {
    fn excludes(&self, root: &Path) -> bool {
        self.actions.iter().any(|(held, action)| match action {
            CheckoutAction::RemoveWorktree => root.starts_with(held),
            CheckoutAction::Git { exclusive, .. } => *exclusive && held == root,
        })
    }
}

/// An operation holding a checkout.
pub(super) enum CheckoutAction {
    Git {
        label: &'static str,
        /// Moves `HEAD` or rewrites the working tree, so no turn may run.
        exclusive: bool,
    },
    RemoveWorktree,
}

impl CheckoutAction {
    const fn label(&self) -> &'static str {
        match self {
            Self::Git { label, .. } => label,
            Self::RemoveWorktree => "remove_worktree",
        }
    }

    const fn excludes_turns(&self) -> bool {
        match self {
            Self::Git { exclusive, .. } => *exclusive,
            Self::RemoveWorktree => true,
        }
    }
}

/// Holds a checkout for an action until dropped.
pub(super) struct Reservation<'a> {
    workspace: &'a Workspace,
    root: PathBuf,
    exclusive: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.workspace
            .reservations
            .lock()
            .actions
            .remove(&self.root);
        self.workspace.git_activity();
        if self.exclusive {
            self.workspace.workers.wake_all();
        }
    }
}

/// A turn's hold on its checkout root until dropped.
pub(super) struct Occupancy<'a> {
    workspace: &'a Workspace,
    root: Option<PathBuf>,
}

impl Drop for Occupancy<'_> {
    fn drop(&mut self) {
        let Some(root) = &self.root else {
            return;
        };
        let mut held = self.workspace.reservations.lock();
        if let Some(count) = held.turns.get_mut(root) {
            *count -= 1;
            if *count == 0 {
                held.turns.remove(root);
            }
        }
    }
}

pub(super) fn checkout_path(repo: &Repository) -> Result<&Path, viz_git::Error> {
    repo.identity()
        .checkout
        .as_deref()
        .ok_or(viz_git::Error::Bare)
}

async fn run_action(
    repo: &Repository,
    action: GitAction,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Option<String>, Error> {
    match action {
        GitAction::CreateBranch { name, start } => {
            repo.create_branch(&name, &start, cancel).await?;
        }
        GitAction::SwitchBranch { name } => {
            repo.switch_branch(&name, CheckoutActivity::Idle, cancel)
                .await?;
        }
        GitAction::Commit { paths, message } => {
            let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            return Ok(Some(repo.commit(&paths, &message, cancel).await?));
        }
        GitAction::Push { remote } => repo.push(&remote, cancel).await?,
        GitAction::Fetch { remote } => repo.fetch(&remote, cancel).await?,
        GitAction::Pull => repo.pull(CheckoutActivity::Idle, cancel).await?,
    }
    Ok(None)
}

const fn action_name(action: &GitAction) -> &'static str {
    match action {
        GitAction::CreateBranch { .. } => "create_branch",
        GitAction::SwitchBranch { .. } => "switch_branch",
        GitAction::Commit { .. } => "commit",
        GitAction::Push { .. } => "push",
        GitAction::Fetch { .. } => "fetch",
        GitAction::Pull => "pull",
    }
}

pub(super) const fn line_stats(stats: viz_git::LineStats) -> GitLineStats {
    GitLineStats {
        added: stats.added,
        removed: stats.removed,
    }
}

pub(super) fn wire_diff(diff: viz_git::Diff) -> GitDiff {
    GitDiff {
        revision: diff.revision.to_string(),
        stats: line_stats(diff.stats),
        truncated: diff.truncated,
        files: diff
            .files
            .into_iter()
            .map(|file| GitDiffFile {
                old_path: file
                    .old_path
                    .map(|path| path.to_string_lossy().into_owned()),
                new_path: file
                    .new_path
                    .map(|path| path.to_string_lossy().into_owned()),
                kind: match file.kind {
                    viz_git::FileKind::Added => GitFileKind::Added,
                    viz_git::FileKind::Deleted => GitFileKind::Deleted,
                    viz_git::FileKind::Modified => GitFileKind::Modified,
                    viz_git::FileKind::Renamed => GitFileKind::Renamed,
                    viz_git::FileKind::Copied => GitFileKind::Copied,
                    viz_git::FileKind::TypeChanged => GitFileKind::TypeChanged,
                    viz_git::FileKind::Conflicted => GitFileKind::Conflicted,
                    viz_git::FileKind::Untracked => GitFileKind::Untracked,
                    viz_git::FileKind::Other => GitFileKind::Other,
                },
                binary: file.binary,
                truncated: file.truncated,
                stats: file.stats.map(line_stats),
                hunks: file
                    .hunks
                    .into_iter()
                    .map(|hunk| GitHunk {
                        old_start: hunk.old_start,
                        old_lines: hunk.old_lines,
                        new_start: hunk.new_start,
                        new_lines: hunk.new_lines,
                        header: String::from_utf8_lossy(&hunk.header).into_owned(),
                        lines: hunk
                            .lines
                            .into_iter()
                            .map(|line| GitDiffLine {
                                origin: line.origin,
                                old_line: line.old_line,
                                new_line: line.new_line,
                                content: String::from_utf8_lossy(&line.content).into_owned(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}
