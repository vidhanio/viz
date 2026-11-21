//! Per-run snapshots of a Git checkout and diffs between those snapshots.

use std::path::Path;

use git2::{Commit, DiffFormat, DiffOptions, ErrorCode, IndexAddOption, Repository, Tree};
use viz_client::protocol::{GitDiff, GitDiffFile};

use super::{
    Workspace,
    sql::{exec, row},
    store,
};
use crate::wire::RpcError;

pub(super) struct Checkpoint {
    cwd: String,
    start: String,
}

impl Workspace {
    pub(super) async fn start_checkpoint(&self, thread: u64, run: u64) -> Option<Checkpoint> {
        match self.capture_start(thread, run).await {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                tracing::warn!(thread, run, %error, "capturing start checkpoint");
                None
            }
        }
    }

    async fn capture_start(&self, thread: u64, run: u64) -> Result<Option<Checkpoint>, RpcError> {
        let cwd = store::binding(&mut *self.db.lock().await, thread)
            .await?
            .cwd;
        let start = format!(
            "refs/viz/checkpoints/{thread}/{run}/{}/start",
            uuid::Uuid::new_v4()
        );
        let checkpoint = Checkpoint { cwd, start };
        let (checkpoint, captured) = tokio::task::spawn_blocking(move || {
            let captured = if let Some(repo) = discover(Path::new(&checkpoint.cwd))? {
                capture(&repo, &checkpoint.start)?;
                true
            } else {
                false
            };
            Ok::<_, RpcError>((checkpoint, captured))
        })
        .await
        .map_err(RpcError::internal)??;
        if !captured {
            return Ok(None);
        }
        exec(
            &mut *self.db.lock().await,
            "UPDATE runs SET checkpoint_start = ?2 WHERE id = ?1",
            [run.into(), checkpoint.start.as_str().into()],
        )
        .await?;
        Ok(Some(checkpoint))
    }

    pub(super) async fn end_checkpoint(&self, run: u64, checkpoint: Checkpoint) -> Option<usize> {
        match self.capture_end(run, checkpoint).await {
            Ok(count) => Some(count),
            Err(error) => {
                tracing::warn!(run, %error, "capturing end checkpoint");
                None
            }
        }
    }

    async fn capture_end(&self, run: u64, checkpoint: Checkpoint) -> Result<usize, RpcError> {
        let (end, count) = tokio::task::spawn_blocking(move || {
            let repo = super::git::open(Path::new(&checkpoint.cwd))?;
            let end = checkpoint.start.trim_end_matches("/start").to_owned() + "/end";
            let after = capture(&repo, &end)?;
            let before = load_tree(&repo, &checkpoint.start)?;
            let mut options = DiffOptions::new();
            options.include_typechange(true);
            let diff = repo.diff_tree_to_tree(Some(&before), Some(&after), Some(&mut options))?;
            Ok::<_, RpcError>((end, diff.deltas().len()))
        })
        .await
        .map_err(RpcError::internal)??;
        exec(
            &mut *self.db.lock().await,
            "UPDATE runs SET checkpoint_end = ?2 WHERE id = ?1",
            [run.into(), end.into()],
        )
        .await?;
        Ok(count)
    }

    pub(super) async fn checkpoint_diff(
        &self,
        thread: u64,
        run: Option<u64>,
        summary: bool,
    ) -> Result<GitDiff, RpcError> {
        let mut db = self.db.lock().await;
        let cwd = store::binding(&mut *db, thread).await?.cwd;
        let (start, end) = if let Some(run) = run {
            let mut record = row(
                &mut *db,
                "SELECT checkpoint_start, checkpoint_end FROM runs WHERE thread = ?1 AND id = ?2",
                [thread.into(), run.into()],
            )
            .await?
            .ok_or_else(|| RpcError::not_found(format!("no run {run} in thread {thread}")))?;
            (record.opt_str(0)?, record.opt_str(1)?)
        } else {
            let first = row(&mut *db, "SELECT checkpoint_start FROM runs WHERE thread = ?1 AND checkpoint_end IS NOT NULL AND hidden = 0 ORDER BY end_entry LIMIT 1", [thread.into()])
                .await?.map(|mut record| record.str(0)).transpose()?;
            let last = row(&mut *db, "SELECT checkpoint_end FROM runs WHERE thread = ?1 AND checkpoint_end IS NOT NULL AND hidden = 0 ORDER BY end_entry DESC LIMIT 1", [thread.into()])
                .await?.map(|mut record| record.str(0)).transpose()?;
            (first, last)
        };
        drop(db);
        let (Some(start), Some(end)) = (start, end) else {
            return Err(RpcError::not_found("no complete Git checkpoint range"));
        };
        tokio::task::spawn_blocking(move || {
            let repo = super::git::open(Path::new(&cwd))?;
            let start = load_tree(&repo, &start)?;
            let end = load_tree(&repo, &end)?;
            diff(&repo, &start, &end, summary)
        })
        .await
        .map_err(RpcError::internal)?
    }
}

fn discover(cwd: &Path) -> Result<Option<Repository>, git2::Error> {
    match Repository::discover(cwd) {
        Ok(repo) if repo.workdir().is_some() => Ok(Some(repo)),
        Ok(_) => Ok(None),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn load_tree<'repo>(
    repo: &'repo Repository,
    reference: &str,
) -> Result<Tree<'repo>, git2::Error> {
    repo.find_reference(reference)?.peel_to_tree()
}

pub(super) fn working_tree(repo: &Repository) -> Result<Tree<'_>, git2::Error> {
    snapshot(repo, head_commit(repo)?.as_ref())
}

fn head_commit(repo: &Repository) -> Result<Option<Commit<'_>>, git2::Error> {
    match repo.head() {
        Ok(head) => head.peel_to_commit().map(Some),
        Err(error) if error.code() == ErrorCode::UnbornBranch => Ok(None),
        Err(error) => Err(error),
    }
}

/// Like t3code's `GitVcsDriver.captureCheckpoint`, stage in a disposable
/// index to include untracked files while preserving HEAD and the real index.
fn snapshot<'repo>(
    repo: &'repo Repository,
    parent: Option<&Commit<'_>>,
) -> Result<Tree<'repo>, git2::Error> {
    const INDEX_STAGE_MASK: u16 = 0x3000;
    let mut index = repo.index()?;
    let staged: Vec<_> = index
        .iter()
        .filter(|entry| entry.flags & INDEX_STAGE_MASK == 0)
        .collect();
    if let Some(parent) = parent {
        index.read_tree(&parent.tree()?)?;
    } else {
        index.clear()?;
    }
    for entry in staged {
        index.add(&entry)?;
    }
    index.update_all(["*"], None)?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    repo.find_tree(index.write_tree_to(repo)?)
}

fn capture<'repo>(repo: &'repo Repository, reference: &str) -> Result<Tree<'repo>, git2::Error> {
    let parent = head_commit(repo)?;
    let tree = snapshot(repo, parent.as_ref())?;
    let signature = git2::Signature::now("viz", "viz@localhost")?;
    let parents: Vec<_> = parent.iter().collect();
    repo.commit(
        Some(reference),
        &signature,
        &signature,
        "viz working tree checkpoint",
        &tree,
        &parents,
    )?;
    Ok(tree)
}

fn diff(
    repo: &Repository,
    start: &Tree<'_>,
    end: &Tree<'_>,
    summary: bool,
) -> Result<GitDiff, RpcError> {
    let mut options = DiffOptions::new();
    options.include_typechange(true);
    let diff = repo.diff_tree_to_tree(Some(start), Some(end), Some(&mut options))?;
    let files = diff
        .deltas()
        .map(|delta| {
            use git2::Delta;
            use viz_client::protocol::GitChange;
            let change = match delta.status() {
                Delta::Added => GitChange::Added,
                Delta::Deleted => GitChange::Deleted,
                Delta::Typechange => GitChange::TypeChanged,
                _ => GitChange::Modified,
            };
            let file = if change == GitChange::Deleted {
                delta.old_file()
            } else {
                delta.new_file()
            };
            GitDiffFile {
                path: file
                    .path()
                    .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
                change,
            }
        })
        .collect();
    let patch = if summary {
        None
    } else {
        let mut patch = Vec::new();
        let mut too_large = false;
        let result = diff.print(DiffFormat::Patch, |_, _, line| {
            if patch.len() + line.content().len() > 4 * 1024 * 1024 {
                too_large = true;
                return false;
            }
            let origin = line.origin();
            if matches!(origin, '+' | '-' | ' ') {
                patch.push(origin as u8);
            }
            patch.extend_from_slice(line.content());
            true
        });
        if too_large {
            return Err(RpcError::invalid(
                "unified diff exceeds 4 MiB; request summary only",
            ));
        }
        result?;
        Some(String::from_utf8_lossy(&patch).into_owned())
    };
    Ok(GitDiff { files, patch })
}
