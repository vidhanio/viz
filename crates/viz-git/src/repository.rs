//! Checkout identity, refs, status and Git operations.

use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use git2::{BranchType, ErrorCode, RepositoryState, Status, StatusOptions};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{CheckoutActivity, Error, blocking, check, guard_checkout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub checkout: Option<PathBuf>,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub main_checkout: Option<PathBuf>,
    pub linked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch { name: String, commit: String },
    Detached { commit: String },
    Unborn { name: String },
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Index and worktree status flags can coexist"
)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub committed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepository {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// Parsed identity only. Remote URLs can contain credentials.
    pub repository: Option<RemoteRepository>,
}

/// Parse an HTTPS, SSH, git, or SCP-style remote without retaining credentials.
#[must_use]
pub fn parse_remote(remote: &str) -> Option<RemoteRepository> {
    let url = if remote.contains("://") {
        Url::parse(remote).ok()?
    } else {
        let (host, path) = remote.split_once(':')?;
        if host.contains('/') || host.contains('\\') || path.starts_with('/') {
            return None;
        }
        Url::parse(&format!("ssh://{host}/{path}")).ok()?
    };
    if !matches!(url.scheme(), "https" | "http" | "ssh" | "git") {
        return None;
    }
    let mut parts = url.path().trim_matches('/').split('/');
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(RemoteRepository {
        host: url.host_str()?.to_owned(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

/// A discovered repository. Clones serialize mutations through the same lock.
#[derive(Debug, Clone)]
pub struct Repository {
    identity: Identity,
    writes: Arc<Mutex<()>>,
}

impl Repository {
    /// Resolve the repository from a directory inside a checkout or a bare
    /// repository.
    ///
    /// # Errors
    /// Returns an error when the repository cannot be discovered or opened.
    pub async fn discover(path: &Path, cancel: &CancellationToken) -> Result<Self, Error> {
        let path = path.to_owned();
        blocking(cancel, move |_| {
            Self::from_git(&git2::Repository::discover(path)?)
        })
        .await
    }

    /// Initialize a repository using Git's configured initial branch.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or Git
    /// initialization fails.
    pub async fn init(path: &Path, cancel: &CancellationToken) -> Result<Self, Error> {
        let path = path.to_owned();
        blocking(cancel, move |_| {
            std::fs::create_dir_all(&path)?;
            run(&path, "init", std::iter::empty::<&str>())?;
            Self::from_git(&git2::Repository::open(path)?)
        })
        .await
    }

    fn from_git(repo: &git2::Repository) -> Result<Self, Error> {
        let checkout = repo.workdir().map(Path::canonicalize).transpose()?;
        let git_dir = repo.path().canonicalize()?;
        let common_dir = repo.commondir().canonicalize()?;
        let linked = repo.is_worktree();
        let main_checkout = if linked {
            git2::Repository::open(&common_dir)?
                .workdir()
                .map(Path::canonicalize)
                .transpose()?
        } else {
            checkout.clone()
        };
        Ok(Self {
            identity: Identity {
                checkout,
                git_dir,
                common_dir,
                main_checkout,
                linked,
            },
            writes: Arc::default(),
        })
    }

    #[must_use]
    pub const fn identity(&self) -> &Identity {
        &self.identity
    }

    pub(super) async fn read<T: Send + 'static>(
        &self,
        cancel: &CancellationToken,
        job: impl FnOnce(&git2::Repository, &CancellationToken) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let path = self.identity.git_dir.clone();
        blocking(cancel, move |cancel| {
            job(&git2::Repository::open(path)?, cancel)
        })
        .await
    }

    pub(super) async fn write<T: Send + 'static>(
        &self,
        cancel: &CancellationToken,
        job: impl FnOnce(&git2::Repository, &Path, &CancellationToken) -> Result<T, Error>
        + Send
        + 'static,
    ) -> Result<T, Error> {
        let writes = Arc::clone(&self.writes);
        self.read(cancel, move |repo, cancel| {
            let _lock = writes
                .lock()
                .map_err(|_| Error::Invalid("repository mutation lock"))?;
            check(cancel)?;
            job(repo, repo.workdir().ok_or(Error::Bare)?, cancel)
        })
        .await
    }

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
        self.write(cancel, move |repo, _, _| {
            guard_conflicts(repo)?;
            validate_branch(&name)?;
            let commit = repo.revparse_single(&start)?.peel_to_commit()?;
            repo.branch(&name, &commit, false)?;
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
        let path = path.to_owned();
        self.write(cancel, move |repo, cwd, cancel| {
            let path = path.canonicalize()?;
            let target = git2::Repository::open(&path)?;
            if !target.is_worktree()
                || target.commondir().canonicalize()? != repo.commondir().canonicalize()?
                || target.workdir() != Some(path.as_path())
                || path == cwd
            {
                return Err(Error::Invalid("linked worktree"));
            }
            if !status_entries(&target, cancel)?.is_empty() {
                return Err(Error::Dirty);
            }
            guard_conflicts(&target)?;
            check(cancel)?;
            run(
                cwd,
                "worktree",
                [OsStr::new("remove"), OsStr::new("--"), path.as_os_str()],
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

    /// Read the index and worktree changes, including untracked paths.
    ///
    /// # Errors
    /// Returns an error if the index, worktree, or status cannot be read.
    pub async fn status(&self, cancel: &CancellationToken) -> Result<Vec<StatusEntry>, Error> {
        self.read(cancel, status_entries).await
    }

    /// Commit the worktree versions of literal paths. Other staged paths stay
    /// staged.
    ///
    /// # Errors
    /// Returns an error for conflicts, invalid selections or a failed Git
    /// commit.
    pub async fn commit(
        &self,
        paths: &[PathBuf],
        message: &str,
        cancel: &CancellationToken,
    ) -> Result<String, Error> {
        if paths.is_empty() || paths.iter().any(|path| !valid_path(path)) {
            return Err(Error::Invalid("selected commit paths"));
        }
        let (paths, message) = (paths.to_vec(), message.to_owned());
        self.write(cancel, move |repo, cwd, cancel| {
            guard_conflicts(repo)?;
            let untracked: Vec<_> = status_entries(repo, cancel)?
                .into_iter()
                .filter(|entry| {
                    entry.untracked && paths.iter().any(|path| entry.path.starts_with(path))
                })
                .map(|entry| entry.path)
                .collect();
            check(cancel)?;
            if !untracked.is_empty() {
                run(
                    cwd,
                    "add",
                    [OsStr::new("-N"), OsStr::new("--")]
                        .into_iter()
                        .chain(untracked.iter().map(|p| p.as_os_str())),
                )?;
            }
            let result = run(
                cwd,
                "commit",
                [
                    OsStr::new("--only"),
                    OsStr::new("-m"),
                    OsStr::new(&message),
                    OsStr::new("--"),
                ]
                .into_iter()
                .chain(paths.iter().map(|p| p.as_os_str())),
            );
            if result.is_err() && !untracked.is_empty() {
                remove_commit_intents(repo, cwd, &untracked)?;
            }
            result?;
            Ok(repo.head()?.peel_to_commit()?.id().to_string())
        })
        .await
    }

    /// Push the current local branch and record its upstream.
    ///
    /// # Errors
    /// Returns an error for missing remotes, detached HEAD or a failed push.
    pub async fn push(&self, remote: &str, cancel: &CancellationToken) -> Result<(), Error> {
        let remote = remote.to_owned();
        self.write(cancel, move |repo, cwd, _| {
            repo.find_remote(&remote)?;
            let head = repo.head()?;
            let name = head.name()?;
            if !name.starts_with("refs/heads/") {
                return Err(Error::Invalid("attached HEAD"));
            }
            run(
                cwd,
                "push",
                ["--set-upstream", "--", &remote, &format!("{name}:{name}")],
            )?;
            Ok(())
        })
        .await
    }

    /// Fetch updates from a configured remote.
    ///
    /// # Errors
    /// Returns an error for missing remotes or a failed fetch.
    pub async fn fetch(&self, remote: &str, cancel: &CancellationToken) -> Result<(), Error> {
        let remote = remote.to_owned();
        self.write(cancel, move |repo, cwd, _| {
            repo.find_remote(&remote)?;
            run(cwd, "fetch", ["--", &remote])?;
            Ok(())
        })
        .await
    }

    /// Fast-forward the checkout from the current branch’s upstream.
    ///
    /// # Errors
    /// Returns an error for active threads, conflicts or a failed fast-forward
    /// pull.
    pub async fn pull(
        &self,
        activity: CheckoutActivity,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        guard_checkout(activity)?;
        self.write(cancel, |repo, cwd, _| {
            guard_conflicts(repo)?;
            run(cwd, "pull", ["--ff-only", "--no-rebase"])?;
            Ok(())
        })
        .await
    }

    /// Count commits unique to the current branch and its upstream.
    ///
    /// # Errors
    /// Returns an error if the upstream or commit graph cannot be read.
    pub async fn ahead_behind(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<AheadBehind>, Error> {
        self.read(cancel, |repo, _| {
            if head_unborn(repo)? || repo.head_detached()? {
                return Ok(None);
            }
            let head = repo.head()?;
            let local = repo.find_branch(head.shorthand()?, BranchType::Local)?;
            let Some(upstream) = upstream(&local)? else {
                return Ok(None);
            };
            let (ahead, behind) = repo.graph_ahead_behind(
                head.peel_to_commit()?.id(),
                upstream.get().peel_to_commit()?.id(),
            )?;
            Ok(Some(AheadBehind { ahead, behind }))
        })
        .await
    }

    /// List configured remote names and parsed repository identities.
    ///
    /// # Errors
    /// Returns an error if remote configuration cannot be read.
    pub async fn remotes(&self, cancel: &CancellationToken) -> Result<Vec<Remote>, Error> {
        self.read(cancel, |repo, _| {
            repo.remotes()?
                .iter()
                .filter_map(|name| name.ok().flatten())
                .map(|name| {
                    let remote = repo.find_remote(name)?;
                    Ok(Remote {
                        name: name.to_owned(),
                        repository: std::str::from_utf8(remote.url_bytes())
                            .ok()
                            .and_then(parse_remote),
                    })
                })
                .collect()
        })
        .await
    }

    /// Read recent commits from the supplied revision or HEAD.
    ///
    /// # Errors
    /// Returns an error when the revision or commit history cannot be read.
    pub async fn log(
        &self,
        revision: Option<&str>,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Vec<Commit>, Error> {
        let revision = revision.map(str::to_owned);
        self.read(cancel, move |repo, cancel| {
            let mut walk = repo.revwalk()?;
            walk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)?;
            if let Some(revision) = revision {
                walk.push(repo.revparse_single(&revision)?.peel_to_commit()?.id())?;
            } else if head_unborn(repo)? {
                return Ok(Vec::new());
            } else {
                walk.push_head()?;
            }
            walk.take(limit)
                .map(|id| {
                    check(cancel)?;
                    let commit = repo.find_commit(id?)?;
                    Ok(Commit {
                        id: commit.id().to_string(),
                        subject: String::from_utf8_lossy(
                            commit.summary_bytes().unwrap_or_default(),
                        )
                        .into_owned(),
                        author: String::from_utf8_lossy(commit.author().name_bytes()).into_owned(),
                        committed_at: commit.time().seconds(),
                    })
                })
                .collect()
        })
        .await
    }
}

pub fn head_unborn(repo: &git2::Repository) -> Result<bool, Error> {
    match repo.head() {
        Ok(_) => Ok(false),
        Err(error) if error.code() == ErrorCode::UnbornBranch => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// Roll back only the intent entries introduced to let Git select untracked
/// paths.
fn remove_commit_intents(
    repo: &git2::Repository,
    cwd: &Path,
    paths: &[PathBuf],
) -> Result<(), Error> {
    let mut index = repo.index()?;
    index.read(true)?;
    let intents: Vec<_> = paths
        .iter()
        .filter(|path| {
            index.get_path(path, 0).is_some_and(|entry| {
                git2::IndexEntryExtendedFlag::from_bits_truncate(entry.flags_extended)
                    .is_intent_to_add()
            })
        })
        .collect();
    if !intents.is_empty() {
        run(
            cwd,
            "update-index",
            [OsStr::new("--force-remove"), OsStr::new("--")]
                .into_iter()
                .chain(intents.into_iter().map(|path| path.as_os_str())),
        )?;
    }
    Ok(())
}

fn upstream<'repo>(branch: &git2::Branch<'repo>) -> Result<Option<git2::Branch<'repo>>, Error> {
    match branch.upstream() {
        Ok(upstream) => Ok(Some(upstream)),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn guard_conflicts(repo: &git2::Repository) -> Result<(), Error> {
    if repo.state() != RepositoryState::Clean || repo.index()?.has_conflicts() {
        return Err(Error::Conflicted);
    }
    Ok(())
}

fn valid_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(name) if name != ".git"))
}

pub fn status_entries(
    repo: &git2::Repository,
    cancel: &CancellationToken,
) -> Result<Vec<StatusEntry>, Error> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    repo.statuses(Some(&mut options))?
        .iter()
        .map(|entry| {
            check(cancel)?;
            let flags = entry.status();
            Ok(StatusEntry {
                path: path_bytes(entry.path_bytes()).into(),
                staged: flags.intersects(
                    Status::INDEX_NEW
                        | Status::INDEX_MODIFIED
                        | Status::INDEX_DELETED
                        | Status::INDEX_RENAMED
                        | Status::INDEX_TYPECHANGE,
                ),
                unstaged: flags.intersects(
                    Status::WT_MODIFIED
                        | Status::WT_DELETED
                        | Status::WT_RENAMED
                        | Status::WT_TYPECHANGE
                        | Status::WT_UNREADABLE,
                ),
                untracked: flags.contains(Status::WT_NEW),
                conflicted: flags.contains(Status::CONFLICTED),
            })
        })
        .collect()
}

#[cfg(unix)]
fn path_bytes(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn path_bytes(bytes: &[u8]) -> OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
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
    let data = run(
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

pub fn git(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_NAMESPACE")
        .arg("--literal-pathspecs")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub fn run(
    cwd: &Path,
    operation: &'static str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Vec<u8>, Error> {
    output(git(cwd).arg(operation).args(args), operation)
}

pub fn output(command: &mut Command, operation: &'static str) -> Result<Vec<u8>, Error> {
    let result = command.output()?;
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(Error::Command {
            operation,
            status: result.status,
        })
    }
}
