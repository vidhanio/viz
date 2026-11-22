//! Which checkout of its project's repository a thread runs in.
//!
//! A thread keeps its project while it moves between the project's own
//! checkout and linked worktrees; its working directory is the same path below
//! whichever checkout root it runs in. Moving never touches files in either
//! checkout and never removes the one left behind.

use std::{path::Path, sync::Arc};

use serde::Deserialize;
use serde_json::{Value, json};
use viz_protocol::{
    BranchList, ListBranches, ListWorktrees, Method, MoveThread, Moved, RemoveWorktree, ThreadId,
    Worktree, WorktreeList, WorktreeTarget,
};

use super::{
    Workspace,
    sql::{exec, row, rows},
    store,
    worktree::{self, Placement},
};
use crate::wire::{RpcError, decode, encode};

impl Workspace {
    pub(super) async fn checkout_call(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method {
            ListBranches::NAME => {
                let ProjectKey { project } = decode(params)?;
                let project = store::project(&mut *self.db.lock().await, &project).await?;
                let branches = worktree::branches(Path::new(&project.path)).await?;
                encode(BranchList { branches })
            }
            ListWorktrees::NAME => {
                let ProjectKey { project } = decode(params)?;
                encode(self.list_worktrees(&project).await?)
            }
            RemoveWorktree::NAME => {
                let Remove { path, force } = decode(params)?;
                self.remove_worktree(&path, force).await?;
                Ok(json!({}))
            }
            MoveThread::NAME => encode(self.move_thread(decode(params)?).await?),
            _ => Err(RpcError::new(
                -32601,
                format!("unknown checkout method {method}"),
            )),
        }
    }

    /// Resolves `target` in `project`'s repository. `source` is the checkout
    /// a new worktree branches from by default, and `title` names its branch.
    pub(super) async fn place(
        &self,
        project: &str,
        source: &Path,
        target: &WorktreeTarget,
        title: &str,
    ) -> Result<Placement, RpcError> {
        match target {
            WorktreeTarget::Local => Ok(Placement {
                cwd: project.to_owned(),
                worktree: None,
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

    async fn list_worktrees(&self, project: &str) -> Result<WorktreeList, RpcError> {
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
            .map(|row| row.u64(0).map(ThreadId))
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
        except: Option<u64>,
    ) -> Result<(), RpcError> {
        if !self.managed(path) {
            return Err(RpcError::invalid(format!(
                "{path} is not a worktree viz created"
            )));
        }
        if let Some(user) = row(
            db,
            "SELECT id FROM threads WHERE worktree = ?1 AND id IS NOT ?2 LIMIT 1",
            [path.into(), except.into()],
        )
        .await?
        {
            return Err(RpcError::invalid(format!(
                "thread {} still runs in {path}",
                user.u64(0)?
            )));
        }
        Ok(())
    }

    async fn remove_worktree(&self, path: &str, force: bool) -> Result<(), RpcError> {
        self.removable(&mut *self.db.lock().await, path, None)
            .await?;
        worktree::remove(Path::new(path), path, force).await?;
        self.changed(None);
        Ok(())
    }

    /// Moves an idle thread. Its harness stops; the next run starts a new
    /// session in the new directory (see [`store::SavedSession::handoff`]).
    async fn move_thread(self: &Arc<Self>, args: MoveThread) -> Result<Moved, RpcError> {
        let thread = args.thread.0;
        let (binding, title) = {
            let mut db = self.db.lock().await;
            let binding = store::binding(&mut db, thread).await?;
            idle(&mut db, thread).await?;
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
            .ok_or_else(|| RpcError::invalid("only a project thread can move to a worktree"))?;
        let placement = self
            .place(&project, Path::new(&binding.cwd), &args.worktree, &title)
            .await?;
        if placement.cwd == binding.cwd && placement.worktree == binding.worktree {
            return Ok(moved(placement));
        }
        let result = self.rebind(thread, &binding, &placement).await;
        if result.is_err() {
            worktree::discard(Path::new(&project), &placement).await;
        }
        result?;
        self.relist(thread);
        self.kick(thread, true);
        Ok(moved(placement))
    }

    /// Points the thread at its new checkout while holding its session slot,
    /// so no run can open a session in between, then stops the old harness.
    async fn rebind(
        &self,
        thread: u64,
        before: &store::Binding,
        placement: &Placement,
    ) -> Result<(), RpcError> {
        let driver = {
            let mut db = self.db.lock().await;
            store::require(&mut db, thread).await?;
            let driver = self.driver(thread);
            drop(db);
            driver
        };
        let mut slot = driver.session.lock().await;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let current = store::binding(&mut tx, thread).await?;
        if current.cwd != before.cwd || current.worktree != before.worktree {
            return Err(RpcError::invalid("the thread moved concurrently"));
        }
        idle(&mut tx, thread).await?;
        let prompted = row(
            &mut tx,
            "SELECT 1 FROM runs WHERE thread = ?1 LIMIT 1",
            [thread.into()],
        )
        .await?
        .is_some();
        let handoff = viz_core::threads::handoff_from(
            prompted,
            current.session.handoff.as_deref(),
            &current.cwd,
        );
        exec(
            &mut tx,
            "UPDATE threads SET cwd = ?2, worktree = ?3, setup_failed = 0,
                session = json_set(session, '$.session_id', NULL, '$.handoff', ?4)
             WHERE id = ?1",
            [
                thread.into(),
                placement.cwd.as_str().into(),
                placement.worktree.as_deref().into(),
                handoff.into(),
            ],
        )
        .await?;
        tx.commit().await?;
        drop(db);
        slot.take();
        drop(slot);
        Ok(())
    }
}

async fn idle(db: &mut crate::workspace::sql::Connection, thread: u64) -> Result<(), RpcError> {
    if row(
        db,
        "SELECT 1 FROM runs WHERE thread = ?1 AND status IN ('queued', 'running') LIMIT 1",
        [thread.into()],
    )
    .await?
    .is_some()
    {
        return Err(RpcError::invalid("stop the thread's runs before moving it"));
    }
    Ok(())
}

fn moved(placement: Placement) -> Moved {
    Moved {
        worktree: placement.worktree.map(store::thread_worktree),
        cwd: placement.cwd,
    }
}

#[derive(Deserialize)]
struct ProjectKey {
    project: String,
}

#[derive(Deserialize)]
struct Remove {
    path: String,
    #[serde(default)]
    force: bool,
}
