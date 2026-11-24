//! Interpret checkout placement using the Git edge and the core's naming rules.

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;
use viz_core::placement::{self, Checkout};
use viz_git::{Repository, WorktreeRemoval};

use super::store;
use crate::error::Error;

const OWNER_MARKER: &str = "viz-managed-checkout";

pub(super) async fn head(path: &Path) -> Option<Option<String>> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || viz_git::fast_head_branch(&path))
        .await
        .ok()?
}

pub(super) async fn repository(path: &str) -> Option<placement::Repository> {
    let repo = Repository::discover(Path::new(path), &CancellationToken::new())
        .await
        .ok()?;
    Some(placement::Repository(
        store::path_text(&repo.identity().common_dir)
            .ok()?
            .to_owned(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placement {
    pub(super) checkout: Checkout,
    pub(super) created: Option<String>,
}

pub(super) struct Listed {
    pub(super) path: String,
    pub(super) branch: Option<String>,
    pub(super) main: bool,
}

pub(super) async fn list(dir: &Path) -> Result<Vec<Listed>, Error> {
    let cancel = CancellationToken::new();
    let repo = discover(dir, &cancel).await?;
    repo.worktrees(&cancel)
        .await?
        .into_iter()
        .filter(|entry| !entry.bare)
        .map(|entry| {
            let main = repo.identity().main_checkout.as_ref() == Some(&entry.path);
            Ok(Listed {
                path: store::path_text(&entry.path)?.to_owned(),
                branch: entry.branch,
                main,
            })
        })
        .collect()
}

pub(super) async fn existing(source: &Path, path: &str) -> Result<Placement, Error> {
    let cancel = CancellationToken::new();
    let source_path = store::directory(source).await?;
    let source = discover(&source_path, &cancel).await?;
    let requested = store::directory(Path::new(path)).await?;
    let target = discover(&requested, &cancel).await?;
    if target.identity().checkout.as_ref() != Some(&requested)
        || target.identity().common_dir != source.identity().common_dir
    {
        return Err(Error::invalid(
            "not a worktree of this project's repository",
        ));
    }
    let prefix = prefix(&source, &source_path)?;
    let cwd = requested.join(prefix);
    Ok(Placement {
        checkout: Checkout {
            cwd: store::path_text(&cwd)?.to_owned(),
            worktree: target
                .identity()
                .linked
                .then(|| store::path_text(&requested).map(str::to_owned))
                .transpose()?,
        },
        created: None,
    })
}

pub(super) async fn create(
    root: &Path,
    source: &Path,
    base: Option<&str>,
    branch: Option<&str>,
    branch_prefix: Option<&str>,
    title: &str,
) -> Result<Placement, Error> {
    let cancel = CancellationToken::new();
    let source = store::directory(source).await?;
    let repo = discover(&source, &cancel).await?;
    let top = repo
        .identity()
        .checkout
        .as_deref()
        .ok_or(viz_git::Error::Bare)?;
    let prefix = prefix(&repo, &source)?;
    let repo_name = top
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let parent = root.join(placement::repository_dir(repo_name));
    tokio::fs::create_dir_all(&parent).await?;
    let parent = tokio::fs::canonicalize(parent).await?;
    if !parent.starts_with(root) {
        return Err(Error::invalid(
            "worktree directory is outside the managed root",
        ));
    }
    let names = repo.branches(&cancel).await?;
    let branch = placement::available_branch(
        title,
        branch,
        branch_prefix,
        names
            .entries
            .iter()
            .filter(|branch| !branch.remote)
            .map(|branch| branch.name.as_str()),
    );
    let path = vacant(&parent, &branch).await?;
    let cwd = store::path_text(&path.join(prefix))?.to_owned();
    let path_text = store::path_text(&path)?.to_owned();
    repo.add_worktree_from(&path, &branch, base.unwrap_or("HEAD"), &cancel)
        .await?;
    if let Err(error) = mark(root, &path, &cancel).await {
        if repo
            .remove_worktree_with(&path, WorktreeRemoval::Clean, &cancel)
            .await
            .is_ok()
        {
            let _ = repo.discard_branch(&branch, &cancel).await;
        }
        return Err(error);
    }
    Ok(Placement {
        checkout: Checkout {
            cwd,
            worktree: Some(path_text),
        },
        created: Some(branch),
    })
}

async fn vacant(parent: &Path, branch: &str) -> Result<PathBuf, Error> {
    let stem = placement::worktree_stem(branch);
    for suffix in 1.. {
        let path = parent.join(placement::numbered(&stem, suffix));
        match tokio::fs::symlink_metadata(&path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
    }
    Err(Error::invalid("no available worktree path"))
}

async fn mark(root: &Path, path: &Path, cancel: &CancellationToken) -> Result<(), Error> {
    let repo = discover(path, cancel).await?;
    tokio::fs::write(
        repo.identity().git_dir.join(OWNER_MARKER),
        store::path_text(root)?,
    )
    .await?;
    Ok(())
}

pub(super) async fn managed(root: &Path, path: &str) -> bool {
    let path = Path::new(path);
    if !path.starts_with(root) {
        return false;
    }
    let cancel = CancellationToken::new();
    let Ok(repo) = discover(path, &cancel).await else {
        return false;
    };
    repo.identity().linked
        && repo.identity().checkout.as_deref() == Some(path)
        && tokio::fs::read_to_string(repo.identity().git_dir.join(OWNER_MARKER))
            .await
            .is_ok_and(|owner| root.to_str() == Some(owner.as_str()))
}

pub(super) async fn remove(root: &Path, path: &str, force: bool) -> Result<(), Error> {
    if !managed(root, path).await {
        return Err(placement::Rejection::Unmanaged(path.to_owned()).into());
    }
    let cancel = CancellationToken::new();
    let path = Path::new(path);
    let repo = discover(path, &cancel).await?;
    let mode = if force {
        WorktreeRemoval::Discard
    } else {
        WorktreeRemoval::Clean
    };
    Ok(repo.remove_worktree_with(path, mode, &cancel).await?)
}

pub(super) async fn discard(root: &Path, source: &Path, placement: &Placement) {
    let (Some(path), Some(branch)) = (&placement.checkout.worktree, &placement.created) else {
        return;
    };
    let result = async {
        let cancel = CancellationToken::new();
        let repo = discover(source, &cancel).await?;
        remove(root, path, true).await?;
        repo.discard_branch(branch, &cancel).await?;
        Ok::<_, Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%path, %error, "rolling back worktree");
    }
}

async fn discover(path: &Path, cancel: &CancellationToken) -> Result<Repository, Error> {
    Ok(Repository::discover(path, cancel).await?)
}

fn prefix<'a>(repo: &Repository, path: &'a Path) -> Result<&'a Path, Error> {
    let root = repo
        .identity()
        .checkout
        .as_deref()
        .ok_or(viz_git::Error::Bare)?;
    path.strip_prefix(root)
        .map_err(|_| Error::invalid("directory is outside the checkout"))
}
