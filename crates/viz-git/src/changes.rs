//! Selected-path commits and remote transfers.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use tokio_util::sync::CancellationToken;

use crate::{
    CheckoutActivity, Error, Repository, check,
    command::run,
    guard_checkout,
    status::{guard_conflicts, status_entries},
};

impl Repository {
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

fn valid_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(name) if name != ".git"))
}
