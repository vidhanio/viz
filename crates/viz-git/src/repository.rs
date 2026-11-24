//! Checkout identity and commit history.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use git2::{ErrorCode, RepositoryOpenFlags};
use tokio_util::sync::CancellationToken;

use crate::{Error, blocking, check, command::run};

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
pub struct Commit {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub committed_at: i64,
}

/// A discovered repository. Clones serialize mutations through the same lock.
#[derive(Debug, Clone)]
pub struct Repository {
    identity: Identity,
    writes: Arc<Mutex<()>>,
}

impl Repository {
    /// Resolve the repository from a directory inside a checkout or a bare
    /// repository. Stop at filesystem boundaries, as Git does by default.
    ///
    /// # Errors
    /// Returns an error when the repository cannot be discovered or opened.
    pub async fn discover(path: &Path, cancel: &CancellationToken) -> Result<Self, Error> {
        let path = path.to_owned();
        blocking(cancel, move |_| {
            let repo = git2::Repository::open_ext(
                path,
                RepositoryOpenFlags::empty(),
                std::iter::empty::<&Path>(),
            )
            .map_err(|error| match error.code() {
                ErrorCode::NotFound => Error::NotRepository,
                _ => Error::Git(error),
            })?;
            Self::from_git(&repo)
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

/// Fast synchronous `HEAD` branch inspection without opening libgit2 or
/// canonicalizing paths.
///
/// Walks upward from `path` to locate `.git` (directory or worktree pointer
/// file) and reads `<git_dir>/HEAD` directly:
/// - `None` — `path` is not inside a Git checkout.
/// - `Some(Some(branch))` — `HEAD` points to `refs/heads/<branch>`.
/// - `Some(None)` — `HEAD` is detached or points outside `refs/heads/`.
#[must_use]
pub fn fast_head_branch(path: &Path) -> Option<Option<String>> {
    let git_dir = locate_git_dir(path)?;
    let head_path = git_dir.join("HEAD");
    let raw = std::fs::read_to_string(head_path).ok()?;
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    Some(trimmed.strip_prefix("ref: refs/heads/").map(str::to_owned))
}

fn locate_git_dir(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    loop {
        let dot_git = current.join(".git");
        if let Ok(meta) = std::fs::symlink_metadata(&dot_git) {
            if meta.is_dir() {
                return Some(dot_git);
            }
            if meta.is_file() {
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let first = text.lines().next()?.trim();
                let target = first.strip_prefix("gitdir:")?.trim();
                let target_path = Path::new(target);
                return Some(if target_path.is_absolute() {
                    target_path.to_path_buf()
                } else {
                    current.join(target_path)
                });
            }
        }
        current = current.parent()?;
    }
}
