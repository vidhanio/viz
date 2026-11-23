//! Snapshots bracket runs and remain scoped to their original checkout.

use std::{path::Path, sync::Arc};

use viz_git::{CheckoutActivity, CheckpointId, Repository};
use viz_protocol::{CheckpointEdge, GitDiff, RestoreCheckpoint, RunId, ThreadId};

use super::{
    Workspace, git,
    sql::{Connection, exec, row},
    store,
};
use crate::error::Error;

impl Workspace {
    /// Checkpoint failures leave the turn runnable, including turns in plain
    /// folders.
    pub(super) async fn checkpoint(&self, thread: ThreadId, run: RunId, edge: CheckpointEdge) {
        match self.save_checkpoint(thread, run, edge).await {
            Ok(()) | Err(Error::Git(viz_git::Error::NotRepository | viz_git::Error::Cancelled)) => {
            }
            Err(error) => {
                tracing::warn!(%thread, %run, ?edge, %error, "could not capture turn checkpoint");
            }
        }
    }

    async fn save_checkpoint(
        &self,
        thread: ThreadId,
        run: RunId,
        edge: CheckpointEdge,
    ) -> Result<(), Error> {
        let repo = self.git_repository(thread).await?;
        let git_dir = store::path_text(&repo.identity().git_dir)?;
        if edge == CheckpointEdge::After {
            let existing = row(&mut *self.db.lock().await,
                "SELECT 1 FROM run_checkpoints WHERE run = ?1 AND git_dir = ?2 AND after_ref IS NULL",
                [run.into(), git_dir.into()]).await?;
            if existing.is_none() {
                return Ok(());
            }
        }
        let snapshot = repo.capture_checkpoint(&self.closing).await?;
        let value = snapshot.to_string();
        let result = {
            let mut db = self.db.lock().await;
            match edge {
                CheckpointEdge::Before => exec(&mut db,
                    "INSERT INTO run_checkpoints (run, git_dir, before_ref) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                    [run.into(), git_dir.into(), value.into()]).await,
                CheckpointEdge::After => exec(&mut db,
                    "UPDATE run_checkpoints SET after_ref = ?3 WHERE run = ?1 AND git_dir = ?2 AND after_ref IS NULL",
                    [run.into(), git_dir.into(), value.into()]).await,
            }
        };
        if !matches!(result, Ok(1)) {
            let cleanup = tokio_util::sync::CancellationToken::new();
            if let Err(error) = repo.delete_checkpoint(snapshot, &cleanup).await {
                tracing::warn!(%run, %error, "could not release unused checkpoint");
            }
        }
        result.map(drop)
    }

    pub(super) async fn checkpoint_diff(
        &self,
        repo: &Repository,
        thread: ThreadId,
        run: Option<RunId>,
    ) -> Result<GitDiff, Error> {
        let git_dir = store::path_text(&repo.identity().git_dir)?;
        let (before, after) = if let Some(run) = run {
            let mut db = self.db.lock().await;
            let before = snapshot(&mut db, thread, run, CheckpointEdge::Before, git_dir).await?;
            let after = snapshot(&mut db, thread, run, CheckpointEdge::After, git_dir).await?;
            drop(db);
            (before, after)
        } else {
            let mut endpoints = row(&mut *self.db.lock().await,
                "SELECT (
                    SELECT c.before_ref FROM run_checkpoints c JOIN runs r ON r.id = c.run
                    WHERE r.thread = ?1 AND c.git_dir = ?2 AND c.after_ref IS NOT NULL ORDER BY r.id LIMIT 1
                ), (
                    SELECT c.after_ref FROM run_checkpoints c JOIN runs r ON r.id = c.run
                    WHERE r.thread = ?1 AND c.git_dir = ?2 AND c.after_ref IS NOT NULL ORDER BY r.id DESC LIMIT 1
                )",
                [thread.into(), git_dir.into()]).await?.ok_or_else(missing_checkpoint)?;
            (
                parse_snapshot(endpoints.opt_str(0)?)?,
                parse_snapshot(endpoints.opt_str(1)?)?,
            )
        };
        Ok(git::wire_diff(
            repo.diff_checkpoints(before, after, &self.closing).await?,
        ))
    }

    pub(super) async fn restore_checkpoint(
        self: &Arc<Self>,
        request: RestoreCheckpoint,
    ) -> Result<(), Error> {
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move { workspace.apply_checkpoint(request).await })
            .await
            .map_err(viz_git::Error::from)?
    }

    async fn apply_checkpoint(&self, request: RestoreCheckpoint) -> Result<(), Error> {
        let mut db = self.db.lock().await;
        let binding = store::binding(&mut db, request.thread).await?;
        let repo = Repository::discover(Path::new(&binding.checkout.cwd), &self.closing).await?;
        let checkout = git::checkout_path(&repo)?;
        self.guard_git_checkout(&mut db, checkout).await?;
        if self.git_actions.lock().await.contains_key(checkout) {
            return Err(viz_git::Error::Busy.into());
        }
        let git_dir = store::path_text(&repo.identity().git_dir)?;
        let target = snapshot(&mut db, request.thread, request.run, request.edge, git_dir).await?;
        let expected = snapshot(
            &mut db,
            request.thread,
            request.expected_run,
            CheckpointEdge::After,
            git_dir,
        )
        .await?;
        let result = repo
            .restore_checkpoint(target, expected, CheckoutActivity::Idle, &self.closing)
            .await;
        drop(db);
        Ok(result?)
    }
}

async fn snapshot(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
    edge: CheckpointEdge,
    git_dir: &str,
) -> Result<CheckpointId, Error> {
    let mut saved = row(
        db,
        "SELECT c.before_ref, c.after_ref FROM run_checkpoints c JOIN runs r ON r.id = c.run
         WHERE r.thread = ?1 AND c.run = ?2 AND c.git_dir = ?3",
        [thread.into(), run.into(), git_dir.into()],
    )
    .await?
    .ok_or_else(missing_checkpoint)?;
    parse_snapshot(saved.opt_str(match edge {
        CheckpointEdge::Before => 0,
        CheckpointEdge::After => 1,
    })?)
}

fn parse_snapshot(saved: Option<String>) -> Result<CheckpointId, Error> {
    saved
        .ok_or_else(missing_checkpoint)?
        .parse()
        .map_err(Error::corrupt)
}

fn missing_checkpoint() -> Error {
    Error::not_found("no checkpoint for this turn in the current checkout")
}
