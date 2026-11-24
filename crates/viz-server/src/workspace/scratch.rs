//! Projectless workspaces and promotion of an existing scratch checkout.

use std::path::{Path, PathBuf};

use viz_core::placement;
use viz_protocol::{ProjectSettings, PromoteThread, PromotedThread, ThreadId};

use super::{
    Workspace,
    reconcile::ScratchPermit,
    sql::{exec, rows},
    store,
};
use crate::error::Error;

/// A private scratch directory provisioned outside any SQLite lock before
/// inserting a projectless thread, holding a [`ScratchPermit`] so periodic
/// orphan reconciliation cannot sweep it before the thread commits.
pub(super) struct PreparedScratch {
    path: PathBuf,
    pub(super) cwd: String,
    _permit: ScratchPermit,
}

impl PreparedScratch {
    /// Removes the empty scratch folder when the thread creation failed or
    /// replayed an earlier request.
    pub(super) async fn discard(self) {
        if let Err(error) = tokio::fs::remove_dir(&self.path).await {
            tracing::warn!(%error, path = %self.path.display(), "could not remove unused scratch folder");
        }
    }
}

impl Workspace {
    /// Provision a private directory outside Git for a new projectless thread
    /// before taking the database write lock.
    pub(super) async fn prepare_scratch(&self, title: &str) -> Result<PreparedScratch, Error> {
        let root = self.scratch_root().await?;
        let date = chrono::DateTime::from_timestamp_millis(store::now())
            .ok_or_else(|| Error::invalid("invalid scratch creation time"))?
            .date_naive()
            .to_string();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let path = root.join(placement::scratch_name(&date, title, &suffix[..8]));
        let cwd = store::path_text(&path)?.to_owned();
        let permit = self.leases.lease_scratch(path.clone());
        tokio::fs::create_dir(&path).await?;
        if let Err(error) = ensure_outside_git(&path, &self.closing).await {
            let _ = tokio::fs::remove_dir(&path).await;
            return Err(error);
        }
        Ok(PreparedScratch {
            path,
            cwd,
            _permit: permit,
        })
    }

    async fn scratch_root(&self) -> Result<PathBuf, Error> {
        let state = store::directory(&self.dir).await?;
        ensure_outside_git(&state, &self.closing).await?;
        let root = state.join("scratch");
        tokio::fs::create_dir_all(&root).await?;
        let canonical = store::directory(&root).await?;
        if canonical != root {
            return Err(Error::invalid(
                "scratch root must be a directory in the state directory",
            ));
        }
        ensure_outside_git(&canonical, &self.closing).await?;
        Ok(canonical)
    }

    pub(super) async fn promote_thread(
        &self,
        args: PromoteThread,
    ) -> Result<PromotedThread, Error> {
        let root = self.scratch_root().await?;
        let thread_id = args.thread;
        let current = self
            .db
            .read(move |conn| {
                let binding = store::binding(conn, thread_id)?;
                Ok(placement::ScratchPromotion::decide(
                    &root,
                    &binding.checkout,
                    binding.project.as_deref(),
                )?)
            })
            .await?;
        let canonical = store::directory(Path::new(&current)).await?;
        if canonical != Path::new(&current) {
            return Err(Error::invalid("scratch folder moved"));
        }
        let requested_name = args.name;
        let current_for_tx = current.clone();
        let (name, now, threads) = self
            .db
            .write_tx(move |tx| {
                let binding = store::binding(tx, thread_id)?;
                if binding.checkout.cwd != current_for_tx || binding.project.is_some() {
                    return Err(Error::invalid("thread placement changed"));
                }
                let title = store::thread_title(tx, thread_id)?;
                let name = placement::ScratchPromotion::name(requested_name, title)?;
                let now = store::now();
                let inserted = exec(
                    tx,
                    "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                    [current_for_tx.as_str().into(), name.as_str().into(), now.into()],
                )?;
                if inserted == 0 {
                    return Err(Error::invalid("scratch folder is already a project"));
                }
                let threads = rows(
                    tx,
                    "SELECT id FROM threads WHERE cwd = ?1 AND project IS NULL ORDER BY id",
                    [current_for_tx.as_str().into()],
                )?
                .map(|row| row.id(0))
                .collect::<Result<Vec<ThreadId>, _>>()?;
                exec(
                    tx,
                    "UPDATE threads SET project = ?1 WHERE cwd = ?1 AND project IS NULL",
                    [current_for_tx.as_str().into()],
                )?;
                Ok((name, now, threads))
            })
            .await?;
        self.project_changed(current.clone());
        for &thread in &threads {
            self.relist(thread);
        }
        let project = store::project_at(current, name, now, ProjectSettings::default()).await;
        Ok(PromotedThread { project, threads })
    }
}

async fn ensure_outside_git(
    path: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), Error> {
    match viz_git::Repository::discover(path, cancel).await {
        Err(viz_git::Error::NotRepository) => Ok(()),
        Ok(_) => Err(Error::invalid(
            "the server state directory for scratch threads must be outside Git",
        )),
        Err(error) => Err(error.into()),
    }
}
