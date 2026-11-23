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

use super::{
    Workspace,
    sql::{Connection, rows},
    store,
};
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
            .git_actions
            .lock()
            .await
            .get(checkout)
            .map(|name| (*name).to_owned())
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

    async fn apply_git_action(&self, request: GitActionRequest) -> Result<GitActionResult, Error> {
        let GitActionRequest { thread, action } = request;
        let mut db = self.db.lock().await;
        let binding = store::binding(&mut db, thread).await?;
        let repo = Repository::discover(Path::new(&binding.checkout.cwd), &self.closing).await?;
        let checkout = checkout_path(&repo)?.to_owned();
        let guarded = matches!(action, GitAction::SwitchBranch { .. } | GitAction::Pull);
        if guarded {
            self.guard_git_checkout(&mut db, &checkout).await?;
        }
        {
            let mut actions = self.git_actions.lock().await;
            let std::collections::hash_map::Entry::Vacant(entry) = actions.entry(checkout.clone())
            else {
                return Err(viz_git::Error::Busy.into());
            };
            entry.insert(action_name(&action));
            drop(actions);
        }
        self.git_activity();
        let result = if guarded {
            let result = run_action(&repo, action, &self.closing).await;
            drop(db);
            result
        } else {
            drop(db);
            run_action(&repo, action, &self.closing).await
        };
        self.git_actions.lock().await.remove(&checkout);
        self.git_activity();
        if guarded && result.is_ok() {
            self.invalidate_checkout_pulls(&checkout);
        }
        result.map(|commit| GitActionResult { commit })
    }

    pub(super) async fn guard_git_checkout(
        &self,
        db: &mut Connection,
        checkout: &Path,
    ) -> Result<(), Error> {
        let active = rows(
            db,
            "SELECT DISTINCT threads.cwd FROM threads JOIN runs ON runs.thread = threads.id WHERE runs.status = 'running'",
            [],
        ).await?.map(|mut row| row.str(0)).collect::<Result<Vec<_>, _>>()?;
        for cwd in active {
            let cwd = tokio::fs::canonicalize(&cwd)
                .await
                .unwrap_or_else(|_| PathBuf::from(cwd));
            if cwd.starts_with(checkout)
                && Repository::discover(&cwd, &self.closing)
                    .await
                    .is_ok_and(|repo| repo.identity().checkout.as_deref() == Some(checkout))
            {
                return Err(viz_git::Error::Busy.into());
            }
        }
        Ok(())
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

pub(super) type GitActions = tokio::sync::Mutex<HashMap<PathBuf, &'static str>>;

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
