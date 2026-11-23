//! Projectless workspaces and promotion of an existing scratch checkout.

use std::path::{Path, PathBuf};

use viz_core::placement;
use viz_protocol::{ProjectSettings, PromoteThread, PromotedThread, ThreadId};

use super::{
    Workspace,
    sql::{Connection, exec, row, rows},
    store,
};
use crate::error::Error;

impl Workspace {
    /// Provision a thread's private working directory before any run is
    /// enqueued.
    pub(super) async fn provision_scratch(
        &self,
        db: &mut Connection,
        thread: ThreadId,
        title: &str,
    ) -> Result<(), Error> {
        let at = store::now();
        let date = chrono::DateTime::from_timestamp_millis(at)
            .ok_or_else(|| Error::invalid("invalid scratch creation time"))?
            .date_naive()
            .to_string();
        let root = self.scratch_root().await?;
        let path = root.join(placement::scratch_name(&date, title, thread));
        tokio::fs::create_dir(&path).await?;
        ensure_outside_git(&path, &self.closing).await?;
        exec(
            db,
            "UPDATE threads SET cwd = ?2 WHERE id = ?1",
            [thread.into(), store::path_text(&path)?.into()],
        )
        .await?;
        Ok(())
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
        let current = {
            let binding = store::binding(&mut *self.db.lock().await, args.thread).await?;
            placement::ScratchPromotion::decide(
                &root,
                &binding.checkout,
                binding.project.as_deref(),
            )?
        };
        let canonical = store::directory(Path::new(&current)).await?;
        if canonical != Path::new(&current) {
            return Err(Error::invalid("scratch folder moved"));
        }
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let binding = store::binding(&mut tx, args.thread).await?;
        if binding.checkout.cwd != current || binding.project.is_some() {
            return Err(Error::invalid("thread placement changed"));
        }
        let title = row(
            &mut tx,
            "SELECT title FROM threads WHERE id = ?1",
            [args.thread.into()],
        )
        .await?
        .ok_or(Error::NoThread(args.thread))?
        .str(0)?;
        let name = placement::ScratchPromotion::name(args.name, title)?;
        let now = store::now();
        let inserted = exec(&mut tx, "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            [current.as_str().into(), name.as_str().into(), now.into()]).await?;
        if inserted == 0 {
            return Err(Error::invalid("scratch folder is already a project"));
        }
        let threads = rows(
            &mut tx,
            "SELECT id FROM threads WHERE cwd = ?1 AND project IS NULL ORDER BY id",
            [current.as_str().into()],
        )
        .await?
        .map(|row| row.id(0))
        .collect::<Result<Vec<ThreadId>, _>>()?;
        exec(
            &mut tx,
            "UPDATE threads SET project = ?1 WHERE cwd = ?1 AND project IS NULL",
            [current.as_str().into()],
        )
        .await?;
        tx.commit().await?;
        drop(db);
        self.changed(None);
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
