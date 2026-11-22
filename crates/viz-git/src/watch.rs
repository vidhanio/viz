//! Debounced invalidation for worktree and repository metadata.

use std::{path::Path, time::Duration};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{Error, Repository, blocking};

/// Receives coalesced status invalidations while the watcher is alive.
pub struct StatusWatch {
    _watcher: RecommendedWatcher,
    events: mpsc::Receiver<Result<(), notify::Error>>,
    debounce: Duration,
}

impl std::fmt::Debug for StatusWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusWatch")
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

impl Repository {
    /// Watch checkout files, the worktree index and shared refs.
    ///
    /// # Errors
    /// Returns an error if the filesystem watcher cannot be created or
    /// registered.
    pub async fn watch(
        &self,
        debounce: Duration,
        cancel: &CancellationToken,
    ) -> Result<StatusWatch, Error> {
        let identity = self.identity().clone();
        blocking(cancel, move |_| {
            let (sender, events) = mpsc::channel(1);
            let metadata = [identity.git_dir.clone(), identity.common_dir.clone()];
            let mut watcher =
                notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = &event
                        && (matches!(event.kind, EventKind::Access(_))
                            || (!event.paths.is_empty()
                                && event
                                    .paths
                                    .iter()
                                    .all(|path| !affects_status(path, &metadata))))
                    {
                        return;
                    }
                    let _ = sender.try_send(event.map(|_| ()));
                })?;
            if let Some(checkout) = &identity.checkout {
                watcher.watch(checkout, RecursiveMode::Recursive)?;
            }
            watcher.watch(&identity.git_dir, RecursiveMode::NonRecursive)?;
            if identity.common_dir != identity.git_dir {
                watcher.watch(&identity.common_dir, RecursiveMode::NonRecursive)?;
            }
            let refs = identity.common_dir.join("refs");
            if refs.is_dir() {
                watcher.watch(&refs, RecursiveMode::Recursive)?;
            }
            Ok(StatusWatch {
                _watcher: watcher,
                events,
                debounce,
            })
        })
        .await
    }
}

fn affects_status(path: &Path, metadata: &[std::path::PathBuf]) -> bool {
    if path
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return false;
    }
    metadata
        .iter()
        .find_map(|dir| path.strip_prefix(dir).ok())
        .is_none_or(|path| {
            matches!(
                path.components()
                    .next()
                    .and_then(|part| part.as_os_str().to_str()),
                None | Some(
                    "HEAD"
                        | "index"
                        | "refs"
                        | "packed-refs"
                        | "config"
                        | "config.worktree"
                        | "MERGE_HEAD"
                        | "CHERRY_PICK_HEAD"
                        | "REVERT_HEAD"
                        | "rebase-merge"
                        | "rebase-apply"
                )
            )
        })
}

/// Allows polling to stop immediately when checkout monitoring is no longer
/// needed.
impl StatusWatch {
    ///
    /// # Errors
    /// Returns an error if cancelled, if the watch closes, or if the watcher
    /// reports a failure.
    pub async fn changed(&mut self, cancel: &CancellationToken) -> Result<(), Error> {
        tokio::select! {
            () = cancel.cancelled() => return Err(Error::Cancelled),
            event = self.events.recv() => event.ok_or(Error::Invalid("closed status watch"))??,
        }
        let quiet = tokio::time::sleep(self.debounce);
        let deadline = tokio::time::sleep(self.debounce.saturating_mul(4));
        tokio::pin!(quiet, deadline);
        loop {
            tokio::select! {
                () = cancel.cancelled() => return Err(Error::Cancelled),
                () = &mut deadline => return Ok(()),
                () = &mut quiet => return Ok(()),
                event = self.events.recv() => {
                    event.ok_or(Error::Invalid("closed status watch"))??;
                    quiet.as_mut().reset(tokio::time::Instant::now() + self.debounce);
                },
            }
        }
    }
}
