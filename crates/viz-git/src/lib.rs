//! Repository operations for the workspace runtime.
//!
//! git2 0.21 provides status, tree/index/worktree diffs with structured
//! patches, rename detection and line statistics, linked worktree discovery,
//! upstream lookup, and ahead/behind counts. gix 0.90 has status, rewrite
//! detection, blob diff statistics, index access, commit/ref creation,
//! worktrees and tracking-ref lookup. Its index/worktree and blob diff APIs
//! require additional composition for patch output. We use git2 for reads, refs
//! and graph traversal, and Git for operations that need checkout filters,
//! hooks, signing, SSH configuration or credential helpers.
//!
//! Cancellation is cooperative for blocking reads, and checked before each
//! mutation begins. Once started, a mutation runs to completion. The caller
//! owns checkout admission and must hold it through destructive operations.

mod branches;
mod changes;
mod checkpoint;
mod clone;
mod command;
mod diff;
mod files;
mod icons;
mod remotes;
mod repository;
mod status;
mod watch;

use std::ffi::OsString;

pub use branches::{Branch, Branches, Worktree, WorktreeRemoval};
pub use checkpoint::CheckpointId;
pub use clone::{ClonePhase, CloneProgress, clone_repository};
pub use command::CommandFailure;
pub use diff::{Diff, DiffFile, DiffLine, DiffTarget, FileKind, Hunk, LineStats, RevisionId};
pub use files::{FileMatch, PathKind, find_files};
pub use icons::{icon_candidate_paths, read_icon};
pub use remotes::{AheadBehind, Remote, RemoteRepository, parse_remote};
pub use repository::{Commit, Head, Identity, Repository};
pub use status::StatusEntry;
use tokio_util::sync::CancellationToken;
pub use watch::StatusWatch;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] git2::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Watch(#[from] notify::Error),
    #[error(transparent)]
    Walk(#[from] ignore::Error),
    #[error("git {operation} failed ({status}): {message}")]
    Command {
        operation: &'static str,
        status: std::process::ExitStatus,
        kind: CommandFailure,
        message: String,
    },
    #[error("operation cancelled")]
    Cancelled,
    #[error("other threads are running in this checkout")]
    Busy,
    #[error("repository operation or unresolved conflicts in progress")]
    Conflicted,
    #[error("worktree has local changes")]
    Dirty,
    #[error("checkout changed since the expected checkpoint")]
    StaleCheckpoint,
    #[error("checkpoint would overwrite an uncaptured path: {0}")]
    CheckpointObstruction(std::path::PathBuf),
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("repository has no working tree")]
    Bare,
    #[error("directory is outside a Git repository")]
    NotRepository,
}

/// The caller's admission decision for a checkout-changing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutActivity {
    Idle,
    OtherThreadsRunning,
}

/// Reject changes to a checkout that is actively shared with other threads.
///
/// # Errors
/// Returns [`Error::Busy`] when other threads are running.
pub const fn guard_checkout(activity: CheckoutActivity) -> Result<(), Error> {
    match activity {
        CheckoutActivity::Idle => Ok(()),
        CheckoutActivity::OtherThreadsRunning => Err(Error::Busy),
    }
}

fn check(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

async fn blocking<T: Send + 'static>(
    cancel: &CancellationToken,
    job: impl FnOnce(&CancellationToken) -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    let cancel = cancel.child_token();
    let _cancel_on_drop = cancel.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        check(&cancel)?;
        job(&cancel)
    })
    .await?
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
