//! Persistent worktree snapshots using a disposable Git index.

use std::{fs, io, path::Path, process::Command, str::FromStr};

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    CheckoutActivity, Diff, Error, Repository, check,
    command::{git, output},
    diff::collect,
    guard_checkout,
    status::guard_conflicts,
};

/// A durable identifier for a hidden ref in the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CheckpointId(Uuid);

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for CheckpointId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl CheckpointId {
    fn reference(self) -> String {
        format!("refs/viz/checkpoints/{}", self.0)
    }
}

impl Repository {
    /// Save the worktree, including nonignored untracked files, without
    /// changing HEAD or the index. Submodules are recorded as Git links. Their
    /// files stay in their own repository.
    ///
    /// # Errors
    /// Returns an error for conflicts, sparse checkout or failed snapshot
    /// creation.
    pub async fn capture_checkpoint(
        &self,
        cancel: &CancellationToken,
    ) -> Result<CheckpointId, Error> {
        self.write(cancel, |repo, cwd, cancel| {
            guard_conflicts(repo)?;
            let index = TempIndex::new(repo, cwd)?;
            let tree = index.capture(repo, cancel)?;
            let id = CheckpointId(Uuid::new_v4());
            check(cancel)?;
            repo.reference(&id.reference(), tree, false, "viz checkpoint")?;
            Ok(id)
        })
        .await
    }

    /// Read a structured patch between saved worktree snapshots.
    ///
    /// # Errors
    /// Returns an error if a checkpoint is missing or the diff cannot be built.
    pub async fn diff_checkpoints(
        &self,
        from: CheckpointId,
        to: CheckpointId,
        cancel: &CancellationToken,
    ) -> Result<Diff, Error> {
        self.read(cancel, move |repo, cancel| {
            let from = repo.find_reference(&from.reference())?.peel_to_tree()?;
            let to = repo.find_reference(&to.reference())?.peel_to_tree()?;
            let mut options = crate::diff::options();
            let diff = repo.diff_tree_to_tree(Some(&from), Some(&to), Some(&mut options))?;
            collect(repo, diff, false, cancel)
        })
        .await
    }

    /// Restore worktree files if the caller holds exclusive checkout admission
    /// and the current tree still equals the expected checkpoint. Git checks
    /// untracked obstructions during checkout. The real index, HEAD, ignored
    /// files and submodule contents stay in place.
    ///
    /// # Errors
    /// Returns an error for an active checkout, stale state, missing
    /// checkpoints or failed checkout.
    pub async fn restore_checkpoint(
        &self,
        target: CheckpointId,
        expected_current: CheckpointId,
        activity: CheckoutActivity,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        guard_checkout(activity)?;
        self.write(cancel, move |repo, cwd, cancel| {
            guard_conflicts(repo)?;
            let expected = repo
                .find_reference(&expected_current.reference())?
                .peel_to_tree()?;
            let target = repo.find_reference(&target.reference())?.peel_to_tree()?;
            let index = TempIndex::new(repo, cwd)?;
            if index.capture(repo, cancel)? != expected.id() {
                return Err(Error::StaleCheckpoint);
            }
            let changed = repo.diff_tree_to_tree(Some(&expected), Some(&target), None)?;
            if changed.deltas().any(|delta| {
                [delta.old_file().mode(), delta.new_file().mode()].contains(&git2::FileMode::Commit)
            }) {
                return Err(Error::Invalid("checkpoint with changed submodule links"));
            }
            for delta in changed.deltas() {
                check(cancel)?;
                if delta.new_file().exists()
                    && let Some(path) = delta.new_file().path()
                {
                    guard_destination(cwd, &expected, path, cancel)?;
                }
            }
            check(cancel)?;
            index.run(
                "read-tree",
                &[
                    "-m",
                    "-u",
                    &expected.id().to_string(),
                    &target.id().to_string(),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Release a snapshot so normal Git garbage collection can reclaim its
    /// objects.
    ///
    /// # Errors
    /// Returns an error if the snapshot ref is missing or cannot be removed.
    pub async fn delete_checkpoint(
        &self,
        id: CheckpointId,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        self.write(cancel, move |repo, _, _| {
            let mut reference = repo.find_reference(&id.reference())?;
            reference.delete()?;
            Ok(())
        })
        .await
    }
}

/// Reject ignored or otherwise unrepresented files that an update could
/// overwrite.
fn guard_destination(
    cwd: &Path,
    expected: &git2::Tree<'_>,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let mut ancestors: Vec<_> = path
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    ancestors.reverse();
    for relative in ancestors {
        let disk = cwd.join(relative);
        let metadata = match disk.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let represented = expected.get_path(relative).is_ok();
        if relative == path {
            if !represented {
                return Err(Error::CheckpointObstruction(relative.to_owned()));
            }
            if metadata.is_dir() {
                guard_directory(cwd, expected, relative, cancel)?;
            }
        } else if !metadata.is_dir() {
            return if represented {
                Ok(())
            } else {
                Err(Error::CheckpointObstruction(relative.to_owned()))
            };
        }
    }
    Ok(())
}

fn guard_directory(
    cwd: &Path,
    expected: &git2::Tree<'_>,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let mut directories = vec![path.to_owned()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(cwd.join(&directory))? {
            check(cancel)?;
            let entry = entry?;
            let relative = directory.join(entry.file_name());
            if expected.get_path(&relative).is_err() {
                return Err(Error::CheckpointObstruction(relative));
            }
            if entry.file_type()?.is_dir() {
                directories.push(relative);
            }
        }
    }
    Ok(())
}

struct TempIndex<'a> {
    dir: tempfile::TempDir,
    cwd: &'a Path,
}

impl<'a> TempIndex<'a> {
    fn new(repo: &git2::Repository, cwd: &'a Path) -> Result<Self, Error> {
        if repo
            .config()?
            .get_bool("core.sparseCheckout")
            .unwrap_or(false)
        {
            return Err(Error::Invalid("checkpoint in a sparse checkout"));
        }
        Ok(Self {
            dir: tempfile::tempdir_in(repo.path())?,
            cwd,
        })
    }

    fn command(&self) -> Command {
        let mut command = git(self.cwd);
        command
            .env("GIT_INDEX_FILE", self.dir.path().join("index"))
            .args([
                "-c",
                "core.splitIndex=false",
                "-c",
                "splitIndex.sharedIndexExpire=never",
                "-c",
                "core.fsmonitor=false",
            ]);
        command
    }

    fn run(&self, operation: &'static str, args: &[&str]) -> Result<Vec<u8>, Error> {
        output(self.command().arg(operation).args(args), operation)
    }

    fn capture(
        &self,
        repo: &git2::Repository,
        cancel: &CancellationToken,
    ) -> Result<git2::Oid, Error> {
        self.run(
            "read-tree",
            &[if crate::repository::head_unborn(repo)? {
                "--empty"
            } else {
                "HEAD"
            }],
        )?;
        check(cancel)?;
        self.run("add", &["--all", "--", "."])?;
        check(cancel)?;
        let bytes = self.run("write-tree", &[])?;
        let id = std::str::from_utf8(&bytes).map_err(|_| Error::Invalid("checkpoint tree id"))?;
        Ok(git2::Oid::from_str(id.trim())?)
    }
}
