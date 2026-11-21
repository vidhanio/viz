//! Git operations for workspace directories.

use std::path::{Path, PathBuf};

use git2::{
    Branch, ErrorCode, Repository, StatusOptions, Worktree, WorktreeAddOptions,
    WorktreePruneOptions,
};
use serde_json::{Value, json};
use viz_client::protocol::{
    CreateGitWorktree, ForkThread, GitBranch, GitBranches, GitSummary, GitWorktree, Launched,
    ListGitBranches, Method, Project, RemoveGitWorktree, RestoreRun, RewindThread, RunDiff,
    ThreadDiff, WorktreeRequest,
};

use super::{
    Workspace,
    sql::{exec, row, rows},
    store,
};
use crate::wire::{RpcError, decode, encode};

pub(super) fn project_summary(mut project: Project) -> Project {
    project.git = summary(Path::new(&project.path));
    project
}

pub(super) fn summary(cwd: &Path) -> Option<GitSummary> {
    let repo = Repository::discover(cwd).ok()?;
    repo.workdir()?;
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .filter(|head| head.is_branch())
        .and_then(|head| head.shorthand().ok())
        .map(str::to_owned)
        .or_else(|| {
            repo.find_reference("HEAD")
                .ok()?
                .symbolic_target()
                .ok()
                .flatten()?
                .strip_prefix("refs/heads/")
                .map(str::to_owned)
        });
    let oid = head.as_ref().and_then(git2::Reference::target);
    let distance = (|| {
        let upstream = Branch::wrap(head?).upstream().ok()?;
        repo.graph_ahead_behind(oid?, upstream.get().target()?).ok()
    })();
    Some(GitSummary {
        branch,
        head: oid.map(|oid| oid.to_string()),
        dirty_files: dirty_files(&repo, false).ok()?,
        ahead: distance.map(|(ahead, _)| ahead),
        behind: distance.map(|(_, behind)| behind),
        linked_worktree: repo.is_worktree(),
    })
}

pub(super) fn dirty_files(repo: &Repository, include_ignored: bool) -> Result<usize, git2::Error> {
    let statuses = repo.statuses(Some(
        StatusOptions::new()
            .include_untracked(true)
            .include_ignored(include_ignored)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true),
    ))?;
    Ok(statuses.len())
}

pub(super) fn open(cwd: &Path) -> Result<Repository, RpcError> {
    let repo = Repository::discover(cwd).map_err(RpcError::from)?;
    if repo.workdir().is_none() {
        return Err(RpcError::invalid("a Git checkout is required"));
    }
    Ok(repo)
}

impl From<git2::Error> for RpcError {
    fn from(error: git2::Error) -> Self {
        Self::invalid(error.message())
    }
}

pub(super) fn branches(cwd: &Path) -> Result<Vec<GitBranch>, RpcError> {
    let repo = open(cwd)?;
    let mut branches = Vec::new();
    for branch in repo.branches(None).map_err(RpcError::from)? {
        let (branch, _) = branch.map_err(RpcError::from)?;
        let reference = branch.get();
        if let Some(head) = reference.target() {
            branches.push(GitBranch {
                reference: reference.name().map_err(RpcError::from)?.to_owned(),
                head: head.to_string(),
            });
        }
    }
    branches.sort_unstable_by(|a, b| a.reference.cmp(&b.reference));
    Ok(branches)
}

/// Owns a freshly created worktree until SQLite commits its registration.
pub(super) struct Provisioned {
    pub(super) checkout: GitWorktree,
    keep: bool,
}

impl Provisioned {
    pub(super) fn keep(mut self) -> GitWorktree {
        self.keep = true;
        self.checkout.clone()
    }
}

impl Drop for Provisioned {
    fn drop(&mut self) {
        if !self.keep
            && let Err(error) = remove(Path::new(&self.checkout.path))
        {
            tracing::warn!(path = %self.checkout.path, %error, "rolling back worktree creation");
        }
    }
}

impl Workspace {
    pub(super) async fn git_call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            ForkThread::NAME => {
                let thread = self.fork(decode(params)?).await?;
                encode(Launched { thread, run: None })
            }
            RestoreRun::NAME => {
                let input: RestoreRun = decode(params)?;
                self.restore(input.thread.0, input.run.0, false).await?;
                Ok(json!({}))
            }
            RewindThread::NAME => {
                let input: RewindThread = decode(params)?;
                self.restore(input.thread.0, input.run.0, true).await?;
                Ok(json!({}))
            }
            RunDiff::NAME => {
                let input: RunDiff = decode(params)?;
                encode(
                    self.checkpoint_diff(input.thread.0, Some(input.run.0), input.summary)
                        .await?,
                )
            }
            ThreadDiff::NAME => {
                let input: ThreadDiff = decode(params)?;
                encode(
                    self.checkpoint_diff(input.thread.0, None, input.summary)
                        .await?,
                )
            }
            ListGitBranches::NAME => {
                let input: ListGitBranches = decode(params)?;
                let project = store::project(&mut *self.db.lock().await, &input.project).await?;
                let branches =
                    tokio::task::spawn_blocking(move || branches(Path::new(&project.path)))
                        .await
                        .map_err(RpcError::internal)??;
                encode(GitBranches { branches })
            }
            CreateGitWorktree::NAME => {
                let input: CreateGitWorktree = decode(params)?;
                let mut db = self.db.lock().await;
                let mut tx = db.transaction().await?;
                let project = store::project(&mut tx, &input.project).await?;
                let pending = self
                    .provision(
                        &mut tx,
                        Path::new(&project.path),
                        input.worktree,
                        project.name,
                    )
                    .await?;
                tx.commit().await?;
                drop(db);
                self.changed(None);
                encode(pending.keep())
            }
            RemoveGitWorktree::NAME => {
                let input: RemoveGitWorktree = decode(params)?;
                self.remove_worktree(&mut *self.db.lock().await, &input.path, None)
                    .await?;
                self.changed(None);
                Ok(json!({}))
            }
            _ => unreachable!("non-Git method"),
        }
    }

    pub(super) async fn provision(
        &self,
        db: &mut dyn toasty::Executor,
        cwd: &Path,
        request: WorktreeRequest,
        title: String,
    ) -> Result<Provisioned, RpcError> {
        let dir = self.worktree_dir.clone();
        let prefix = self.worktree_branch_prefix.clone();
        let cwd = cwd.to_owned();
        let pending =
            tokio::task::spawn_blocking(move || create(&cwd, &dir, &prefix, request, &title))
                .await
                .map_err(RpcError::internal)??;
        exec(
            db,
            "INSERT INTO git_worktrees (path, branch) VALUES (?1, ?2)",
            [
                pending.checkout.path.as_str().into(),
                pending.checkout.branch.as_str().into(),
            ],
        )
        .await?;
        Ok(pending)
    }

    pub(super) async fn remove_worktree(
        &self,
        db: &mut dyn toasty::Executor,
        path: &str,
        deleting: Option<u64>,
    ) -> Result<(), RpcError> {
        if row(
            db,
            "SELECT 1 FROM git_worktrees WHERE path = ?1",
            [path.into()],
        )
        .await?
        .is_none()
        {
            return Err(RpcError::not_found("no viz worktree at that path"));
        }
        for mut binding in rows(db, "SELECT id, cwd FROM threads", []).await? {
            if Some(binding.u64(0)?) != deleting && Path::new(&binding.str(1)?).starts_with(path) {
                return Err(RpcError::invalid("worktree is in use by another thread"));
            }
        }
        let checkout = PathBuf::from(path);
        tokio::task::spawn_blocking(move || remove(&checkout))
            .await
            .map_err(RpcError::internal)??;
        exec(
            db,
            "DELETE FROM git_worktrees WHERE path = ?1",
            [path.into()],
        )
        .await?;
        Ok(())
    }
}

fn slug(title: &str) -> String {
    let words = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty());
    let mut result = String::new();
    for word in words {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&word.to_ascii_lowercase());
        if result.len() >= 48 {
            result.truncate(48);
            break;
        }
    }
    if result.is_empty() {
        "thread".into()
    } else {
        result
    }
}

/// Like t3code's `ThreadLaunchService`, resolve the base and create a new
/// branch on the destination server before binding the thread to the checkout.
fn create(
    cwd: &Path,
    dir: &Path,
    prefix: &str,
    request: WorktreeRequest,
    title: &str,
) -> Result<Provisioned, RpcError> {
    let repo = open(cwd)?;
    let base = repo
        .revparse_single(request.base_ref.as_deref().unwrap_or("HEAD"))
        .and_then(|object| object.peel_to_commit())
        .map_err(RpcError::from)?;
    std::fs::create_dir_all(dir).map_err(RpcError::internal)?;
    let dir = dir.canonicalize().map_err(RpcError::internal)?;
    if dir.starts_with(repo.commondir()) {
        return Err(RpcError::invalid(
            "worktree directory must be outside the Git directory",
        ));
    }
    let id = format!("viz-{}", uuid::Uuid::new_v4());
    let path = dir.join(&id);
    let path_text = store::path_text(&path)?.to_owned();
    let branch = if let Some(branch) = request.branch {
        repo.branch(&branch, &base, false).map_err(RpcError::from)?
    } else {
        let stem = format!("{prefix}{}", slug(title));
        let mut suffix = 1;
        loop {
            let name = if suffix == 1 {
                stem.clone()
            } else {
                format!("{stem}-{suffix}")
            };
            match repo.branch(&name, &base, false) {
                Ok(branch) => break branch,
                Err(error) if error.code() == ErrorCode::Exists => suffix += 1,
                Err(error) => return Err(RpcError::from(error)),
            }
        }
    };
    let branch_name = branch.get().shorthand().map_err(RpcError::from)?.to_owned();
    let mut options = WorktreeAddOptions::new();
    options.reference(Some(branch.get()));
    if let Err(error) = repo.worktree(&id, &path, Some(&options)) {
        let mut branch = branch;
        if !path.exists()
            && let Err(cleanup) = branch.delete()
        {
            tracing::warn!(%cleanup, branch = %branch_name, "rolling back worktree branch");
        }
        return Err(RpcError::from(error));
    }
    Ok(Provisioned {
        checkout: GitWorktree {
            path: path_text,
            branch: branch_name,
        },
        keep: false,
    })
}

fn remove(path: &Path) -> Result<(), RpcError> {
    let repo = Repository::open(path).map_err(RpcError::from)?;
    let worktree = Worktree::open_from_repository(&repo).map_err(RpcError::from)?;
    if worktree.path() != path
        || !worktree
            .name()
            .map_err(RpcError::from)?
            .is_some_and(|name| {
                name.starts_with("viz-") && path.file_name().is_some_and(|file| file == name)
            })
    {
        return Err(RpcError::invalid(
            "checkout no longer matches the registered worktree",
        ));
    }
    if dirty_files(&repo, true).map_err(RpcError::from)? != 0 {
        return Err(RpcError::invalid("worktree has changes or untracked files"));
    }
    worktree
        .prune(Some(
            WorktreePruneOptions::new().valid(true).working_tree(true),
        ))
        .map_err(RpcError::from)
}
