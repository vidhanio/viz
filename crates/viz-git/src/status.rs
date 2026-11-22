//! Index and worktree status, including ongoing Git operations.

use std::path::PathBuf;

use git2::{RepositoryState, Status, StatusOptions};
use tokio_util::sync::CancellationToken;

use crate::{Error, Repository, check, path_bytes};

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

impl Repository {
    /// Read the index and worktree changes, including untracked paths.
    ///
    /// # Errors
    /// Returns an error if the index, worktree, or status cannot be read.
    pub async fn status(&self, cancel: &CancellationToken) -> Result<Vec<StatusEntry>, Error> {
        self.read(cancel, status_entries).await
    }
}

pub fn guard_conflicts(repo: &git2::Repository) -> Result<(), Error> {
    if repo.state() != RepositoryState::Clean || repo.index()?.has_conflicts() {
        return Err(Error::Conflicted);
    }
    Ok(())
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
