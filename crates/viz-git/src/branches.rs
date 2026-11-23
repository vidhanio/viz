//! Branches and the linked worktree registry.

use std::{
    collections::HashSet,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use git2::{BranchType, ErrorCode};
use tokio_util::sync::CancellationToken;

use crate::{
    CheckoutActivity, Error, Head, Repository, check,
    command::{run, run_read},
    guard_checkout, path_bytes,
    repository::head_unborn,
    status::{guard_conflicts, status_entries},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
    pub commit: String,
    pub committed_at: i64,
    pub checked_out_elsewhere: bool,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branches {
    pub entries: Vec<Branch>,
    /// Cached remote default or the configured initial branch when present
    /// locally.
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub locked: bool,
    pub prunable: bool,
    pub bare: bool,
}

/// Whether local modifications prevent linked worktree deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeRemoval {
    Clean,
    Discard,
}

impl Repository {
    /// Inspect the attached, detached, or unborn HEAD.
    ///
    /// # Errors
    /// Returns an error if HEAD cannot be read or resolved.
    pub async fn head(&self, cancel: &CancellationToken) -> Result<Head, Error> {
        self.read(cancel, |repo, _| {
            if head_unborn(repo)? {
                return Ok(Head::Unborn {
                    name: repo
                        .find_reference("HEAD")?
                        .symbolic_target()?
                        .and_then(|name| name.strip_prefix("refs/heads/"))
                        .ok_or(Error::Invalid("unborn HEAD"))?
                        .to_owned(),
                });
            }
            let head = repo.head()?;
            let commit = head.peel_to_commit()?.id().to_string();
            if repo.head_detached()? {
                Ok(Head::Detached { commit })
            } else {
                Ok(Head::Branch {
                    name: head.shorthand()?.to_owned(),
                    commit,
                })
            }
        })
        .await
    }

    /// List local and remote-tracking branches with upstream and checkout
    /// information.
    ///
    /// # Errors
    /// Returns an error if references or worktree metadata cannot be read.
    pub async fn branches(&self, cancel: &CancellationToken) -> Result<Branches, Error> {
        self.read(cancel, |repo, cancel| {
            let elsewhere: HashSet<_> = list_worktrees(repo)?
                .into_iter()
                .filter(|entry| repo.workdir().is_none_or(|cwd| cwd != entry.path))
                .filter_map(|entry| entry.branch)
                .collect();
            let mut entries = Vec::new();
            for item in repo.branches(None)? {
                check(cancel)?;
                let (branch, kind) = item?;
                if branch.get().symbolic_target_bytes().is_some() {
                    continue;
                }
                let name = branch
                    .name()?
                    .ok_or(Error::Invalid("branch name"))?
                    .to_owned();
                let commit = branch.get().peel_to_commit()?;
                let remote = kind == BranchType::Remote;
                let upstream = if remote {
                    None
                } else {
                    upstream(&branch)?
                        .map(|branch| branch.get().shorthand().map(str::to_owned))
                        .transpose()?
                };
                entries.push(Branch {
                    checked_out_elsewhere: !remote && elsewhere.contains(&name),
                    name,
                    remote,
                    upstream,
                    commit: commit.id().to_string(),
                    committed_at: commit.time().seconds(),
                });
            }
            entries.sort_by(|a, b| (a.remote, &a.name).cmp(&(b.remote, &b.name)));
            let mut remotes: Vec<_> = repo
                .remotes()?
                .iter()
                .filter_map(|name| name.ok().flatten())
                .map(str::to_owned)
                .collect();
            remotes.sort_by(|a, b| (a != "origin", a).cmp(&(b != "origin", b)));
            let default = remotes.iter().find_map(|remote| {
                let reference = repo
                    .find_reference(&format!("refs/remotes/{remote}/HEAD"))
                    .ok()?;
                reference
                    .symbolic_target()
                    .ok()??
                    .strip_prefix(&format!("refs/remotes/{remote}/"))
                    .map(str::to_owned)
            });
            let default = default.or_else(|| {
                let name = repo.config().ok()?.get_string("init.defaultBranch").ok()?;
                entries
                    .iter()
                    .any(|branch| !branch.remote && branch.name == name)
                    .then_some(name)
            });
            Ok(Branches { entries, default })
        })
        .await
    }

    /// Create a local branch from a commit, tag or branch revision.
    ///
    /// # Errors
    /// Returns an error on conflicts, invalid names, missing start commits, or
    /// an existing branch.
    pub async fn create_branch(
        &self,
        name: &str,
        start: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let (name, start) = (name.to_owned(), start.to_owned());
        self.write(cancel, move |repo, cwd, _| {
            guard_conflicts(repo)?;
            validate_branch(&name)?;
            let commit = repo.revparse_single(&start)?.peel_to_commit()?;
            run(cwd, "branch", ["--", &name, &commit.id().to_string()])?;
            Ok(())
        })
        .await
    }

    /// Switch to an existing local branch using Git’s safe checkout.
    ///
    /// # Errors
    /// Returns an error on active threads, conflicts, missing branches or
    /// checkout failure.
    pub async fn switch_branch(
        &self,
        name: &str,
        activity: CheckoutActivity,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        guard_checkout(activity)?;
        let name = name.to_owned();
        self.write(cancel, move |repo, cwd, _| {
            guard_conflicts(repo)?;
            validate_branch(&name)?;
            repo.find_branch(&name, BranchType::Local)?;
            run(cwd, "switch", ["--no-guess", "--", &name])?;
            Ok(())
        })
        .await
    }

    /// List the main checkout and registered linked worktrees.
    ///
    /// # Errors
    /// Returns an error if Git cannot enumerate the registered worktrees.
    pub async fn worktrees(&self, cancel: &CancellationToken) -> Result<Vec<Worktree>, Error> {
        self.read(cancel, |repo, _| list_worktrees(repo)).await
    }

    /// Check out an available local branch in a new linked worktree.
    ///
    /// # Errors
    /// Returns an error if the branch is unavailable or Git cannot create the
    /// worktree.
    pub async fn add_worktree(
        &self,
        path: &Path,
        branch: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let (path, branch) = (path.to_owned(), branch.to_owned());
        self.write(cancel, move |repo, cwd, _| {
            validate_branch(&branch)?;
            repo.find_branch(&branch, BranchType::Local)?;
            if !path.is_absolute() {
                return Err(Error::Invalid("absolute worktree path"));
            }
            run(
                cwd,
                "worktree",
                [
                    OsStr::new("add"),
                    OsStr::new("--"),
                    path.as_os_str(),
                    OsStr::new(&branch),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Create a linked worktree and a new branch without setting an upstream.
    ///
    /// # Errors
    /// Returns an error for an invalid path, branch or base, or Git failure.
    pub async fn add_worktree_from(
        &self,
        path: &Path,
        branch: &str,
        base: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let (path, branch, base) = (path.to_owned(), branch.to_owned(), base.to_owned());
        self.write(cancel, move |repo, cwd, _| {
            validate_branch(&branch)?;
            if !path.is_absolute() {
                return Err(Error::Invalid("absolute worktree path"));
            }
            let oid = repo
                .revparse_single(&base)?
                .peel_to_commit()?
                .id()
                .to_string();
            run(
                cwd,
                "worktree",
                [
                    OsStr::new("add"),
                    OsStr::new("--no-track"),
                    OsStr::new("-b"),
                    OsStr::new(&branch),
                    OsStr::new("--"),
                    path.as_os_str(),
                    OsStr::new(&oid),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Delete an unused branch, for rolling back an abandoned checkout
    /// creation.
    ///
    /// # Errors
    /// Returns an error if Git refuses the deletion.
    pub async fn discard_branch(
        &self,
        branch: &str,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let branch = branch.to_owned();
        self.write(cancel, move |_, cwd, _| {
            validate_branch(&branch)?;
            run(cwd, "branch", ["-D", "--", &branch])?;
            Ok(())
        })
        .await
    }

    /// Remove a clean linked checkout belonging to this repository.
    ///
    /// # Errors
    /// Returns an error for dirty, locked or unrelated worktrees, or failed
    /// removal.
    pub async fn remove_worktree(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        self.remove_worktree_with(path, WorktreeRemoval::Clean, cancel)
            .await
    }

    /// Remove a linked worktree using the specified cleanliness policy.
    ///
    /// # Errors
    /// Returns an error if the worktree belongs to another repository or Git
    /// refuses removal.
    pub async fn remove_worktree_with(
        &self,
        path: &Path,
        mode: WorktreeRemoval,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let path = path.to_owned();
        self.write(cancel, move |repo, _, cancel| {
            let path = path.canonicalize()?;
            let target = git2::Repository::open(&path)?;
            if !target.is_worktree()
                || target.commondir().canonicalize()? != repo.commondir().canonicalize()?
                || target.workdir() != Some(path.as_path())
            {
                return Err(Error::Invalid("linked worktree"));
            }
            let mut args = vec![OsStr::new("remove")];
            match mode {
                WorktreeRemoval::Clean => {
                    if !status_entries(&target, cancel)?.is_empty() {
                        return Err(Error::Dirty);
                    }
                    guard_conflicts(&target)?;
                }
                WorktreeRemoval::Discard => args.push(OsStr::new("--force")),
            }
            check(cancel)?;
            let main = git2::Repository::open(repo.commondir())?;
            args.extend([OsStr::new("--"), path.as_os_str()]);
            run(
                main.workdir().unwrap_or_else(|| main.path()),
                "worktree",
                args,
            )?;
            Ok(())
        })
        .await
    }

    /// Prune stale worktree registrations using Git’s expiry policy.
    ///
    /// # Errors
    /// Returns an error if Git cannot prune the worktree registry.
    pub async fn prune_worktrees(&self, cancel: &CancellationToken) -> Result<(), Error> {
        self.write(cancel, |_, cwd, _| {
            run(cwd, "worktree", ["prune"])?;
            Ok(())
        })
        .await
    }
}

pub fn upstream<'repo>(branch: &git2::Branch<'repo>) -> Result<Option<git2::Branch<'repo>>, Error> {
    match branch.upstream() {
        Ok(upstream) => Ok(Some(upstream)),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_branch(name: &str) -> Result<(), Error> {
    if name.starts_with('-')
        || name == "HEAD"
        || !git2::Reference::is_valid_name(&format!("refs/heads/{name}"))
    {
        return Err(Error::Invalid("branch name"));
    }
    Ok(())
}

fn list_worktrees(repo: &git2::Repository) -> Result<Vec<Worktree>, Error> {
    let data = run_read(
        repo.workdir().unwrap_or_else(|| repo.path()),
        "worktree",
        ["list", "--porcelain", "-z"],
    )?;
    let entries = data.split(|byte| *byte == 0).collect::<Vec<_>>();
    entries
        .split(|entry| entry.is_empty())
        .filter(|entry| !entry.is_empty())
        .map(parse_worktree)
        .collect()
}

fn parse_worktree(lines: &[&[u8]]) -> Result<Worktree, Error> {
    let path = lines
        .first()
        .and_then(|line| line.strip_prefix(b"worktree "))
        .ok_or(Error::Invalid("worktree output"))?;
    let value = |prefix: &[u8]| {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    };
    let flag = |key: &[u8]| {
        lines.iter().any(|line| {
            *line == key
                || line
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with(b" "))
        })
    };
    Ok(Worktree {
        path: path_bytes(path).into(),
        branch: value(b"branch refs/heads/"),
        commit: value(b"HEAD ").filter(|id| id.bytes().any(|byte| byte != b'0')),
        locked: flag(b"locked"),
        prunable: flag(b"prunable"),
        bare: flag(b"bare"),
    })
}
