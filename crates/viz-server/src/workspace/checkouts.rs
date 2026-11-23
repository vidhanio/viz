//! Which checkout of its project's repository a thread runs in.
//!
//! A thread keeps its project while it moves between the project's own
//! checkout and linked worktrees; its working directory is the same path below
//! whichever checkout root it runs in. Moving never touches files in either
//! checkout and never removes the one left behind.

use std::{path::Path, sync::Arc};

use viz_core::{
    placement::{self, Checkout, Placed, Site},
    threads,
};
use viz_protocol::{MoveThread, Moved, ThreadId, Worktree, WorktreeList, WorktreeTarget};

use super::{
    Workspace, runs,
    sql::{exec, row, rows},
    store,
    worktree::{self, Placement},
};
use crate::error::Error;

impl Workspace {
    /// Whether a thread's checkout is a place in the project it is filed
    /// under.
    pub(super) async fn within(&self, binding: &store::Binding) -> bool {
        let Some(project) = &binding.project else {
            return false;
        };
        let cwd = &binding.checkout.cwd;
        if cwd == project {
            return true;
        }
        let (here, there) = tokio::join!(worktree::repository(cwd), worktree::repository(project));
        placement::within(
            Site {
                path: cwd,
                repository: here.as_ref(),
            },
            Site {
                path: project,
                repository: there.as_ref(),
            },
        )
    }

    /// Resolves `target` in `project`'s repository. `source` is the checkout
    /// a new worktree branches from by default, and `title` names its branch.
    pub(super) async fn place(
        &self,
        project: &str,
        source: &Path,
        target: &WorktreeTarget,
        title: &str,
    ) -> Result<Placement, Error> {
        match target {
            WorktreeTarget::Local => Ok(Placement {
                checkout: Checkout {
                    cwd: project.to_owned(),
                    worktree: None,
                },
                created: None,
            }),
            WorktreeTarget::Existing { path } => worktree::existing(source, path).await,
            WorktreeTarget::New { base, branch } => {
                worktree::create(
                    &self.worktrees,
                    source,
                    base.as_deref(),
                    branch.as_deref(),
                    title,
                )
                .await
            }
        }
    }

    pub(super) fn managed(&self, path: &str) -> bool {
        Path::new(path).starts_with(&self.worktrees)
    }

    pub(super) async fn list_worktrees(&self, project: &str) -> Result<WorktreeList, Error> {
        let project = store::project(&mut *self.db.lock().await, project).await?;
        let listed = worktree::list(Path::new(&project.path)).await?;
        let mut db = self.db.lock().await;
        let mut worktrees = Vec::with_capacity(listed.len());
        for checkout in listed {
            let threads = if checkout.main {
                rows(
                    &mut db,
                    "SELECT id FROM threads WHERE project = ?1 AND worktree IS NULL ORDER BY id",
                    [project.path.as_str().into()],
                )
                .await?
            } else {
                rows(
                    &mut db,
                    "SELECT id FROM threads WHERE worktree = ?1 ORDER BY id",
                    [checkout.path.as_str().into()],
                )
                .await?
            }
            .map(|row| row.id(0))
            .collect::<Result<_, _>>()?;
            worktrees.push(Worktree {
                managed: !checkout.main && self.managed(&checkout.path),
                path: checkout.path,
                branch: checkout.branch,
                main: checkout.main,
                threads,
            });
        }
        drop(db);
        Ok(WorktreeList { worktrees })
    }

    /// Checks that `path` is viz's own worktree and that no thread other
    /// than `except` runs in it.
    pub(super) async fn removable(
        &self,
        db: &mut crate::workspace::sql::Connection,
        path: &str,
        except: Option<ThreadId>,
    ) -> Result<(), Error> {
        let user = row(
            db,
            "SELECT id FROM threads WHERE worktree = ?1 AND id IS NOT ?2 LIMIT 1",
            [path.into(), except.into()],
        )
        .await?
        .map(|row| row.id(0))
        .transpose()?;
        Ok(placement::removable(path, self.managed(path), user)?)
    }

    pub(super) async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), Error> {
        self.removable(&mut *self.db.lock().await, path, None)
            .await?;
        worktree::remove(Path::new(path), path, force).await?;
        self.changed(None);
        Ok(())
    }

    /// Moves an idle thread. Its harness stops; the next run starts a new
    /// session in the new directory (see [`store::SavedSession::handoff`]).
    pub(super) async fn move_thread(self: &Arc<Self>, args: MoveThread) -> Result<Moved, Error> {
        let thread = args.thread;
        let (binding, title) = {
            let mut db = self.db.lock().await;
            let binding = store::binding(&mut db, thread).await?;
            runs::idle(&mut db, thread).await?;
            let title = row(
                &mut db,
                "SELECT title FROM threads WHERE id = ?1",
                [thread.into()],
            )
            .await?
            .map(|mut row| row.str(0))
            .transpose()?
            .unwrap_or_default();
            drop(db);
            (binding, title)
        };
        let project = binding
            .project
            .clone()
            .ok_or(threads::Rejection::NoProject)?;
        let source = Placed {
            project: Some(&project),
            checkout: &binding.checkout,
            within: self.within(&binding).await,
        }
        .source(&project)
        .to_owned();
        let placement = self
            .place(&project, Path::new(&source), &args.worktree, &title)
            .await?;
        if placement.checkout == binding.checkout {
            return Ok(moved(placement.checkout));
        }
        let result = self.rebind(thread, binding, placement.clone()).await;
        if result.is_err() {
            worktree::discard(Path::new(&project), &placement).await;
        }
        result?;
        self.relist(thread);
        Ok(moved(placement.checkout))
    }

    async fn rebind(
        self: &Arc<Self>,
        thread: ThreadId,
        before: store::Binding,
        placement: Placement,
    ) -> Result<(), Error> {
        let driver = {
            let mut db = self.db.lock().await;
            store::require(&mut db, thread).await?;
            let driver = self.driver(thread)?;
            drop(db);
            driver
        };
        driver.rebind(before, placement).await
    }

    /// Only the session owner may change the binding while the thread is idle.
    pub(super) async fn apply_checkout(
        &self,
        thread: ThreadId,
        before: &store::Binding,
        placement: &Placement,
    ) -> Result<(), Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let current = store::binding(&mut tx, thread).await?;
        if current.checkout != before.checkout {
            return Err(threads::Rejection::Moved.into());
        }
        runs::idle(&mut tx, thread).await?;
        let prompted = row(
            &mut tx,
            "SELECT 1 FROM runs WHERE thread = ?1 LIMIT 1",
            [thread.into()],
        )
        .await?
        .is_some();
        let handoff = threads::handoff_from(
            prompted,
            current.session.handoff.as_deref(),
            &current.checkout.cwd,
        );
        exec(
            &mut tx,
            "UPDATE threads SET cwd = ?2, worktree = ?3, setup_failed = 0,
                session = json_set(session, '$.session_id', NULL, '$.handoff', ?4)
             WHERE id = ?1",
            [
                thread.into(),
                placement.checkout.cwd.as_str().into(),
                placement.checkout.worktree.as_deref().into(),
                handoff.into(),
            ],
        )
        .await?;
        tx.commit().await?;
        drop(db);
        Ok(())
    }
}

fn moved(checkout: Checkout) -> Moved {
    Moved {
        worktree: checkout.worktree.map(store::thread_worktree),
        cwd: checkout.cwd,
    }
}
