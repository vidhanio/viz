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
    store::{self, Handoff},
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
                let (_, _, preferences) =
                    store::preferences(&mut *self.db.lock().await, project).await?;
                worktree::create(
                    &self.worktrees,
                    source,
                    base.as_deref()
                        .or(preferences.settings.base_branch.as_deref()),
                    branch.as_deref(),
                    preferences.settings.branch_prefix.as_deref(),
                    title,
                )
                .await
            }
        }
    }

    pub(super) async fn list_worktrees(&self, project: &str) -> Result<WorktreeList, Error> {
        let project = store::project(&mut *self.db.lock().await, project).await?;
        let listed = worktree::list(Path::new(&project.path)).await?;
        let filed = rows(
            &mut *self.db.lock().await,
            "SELECT id, cwd FROM threads WHERE project = ?1 OR worktree IS NOT NULL ORDER BY id",
            [project.path.as_str().into()],
        )
        .await?;
        let mut by_checkout = std::collections::HashMap::<String, Vec<ThreadId>>::new();
        let mut resolved = std::collections::HashMap::new();
        for mut row in filed {
            let cwd = row.str(1)?;
            let root = match resolved.entry(cwd) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let repo =
                        viz_git::Repository::discover(Path::new(entry.key()), &self.closing).await;
                    let checkout = repo.ok().and_then(|repo| {
                        repo.identity()
                            .checkout
                            .as_deref()
                            .and_then(Path::to_str)
                            .map(str::to_owned)
                    });
                    entry.insert(checkout)
                }
            };
            if let Some(path) = root {
                by_checkout
                    .entry(path.clone())
                    .or_default()
                    .push(row.id(0)?);
            }
        }
        let mut worktrees = Vec::with_capacity(listed.len());
        for checkout in listed {
            let threads = by_checkout.remove(&checkout.path).unwrap_or_default();
            worktrees.push(Worktree {
                managed: !checkout.main && worktree::managed(&self.worktrees, &checkout.path).await,
                path: checkout.path,
                branch: checkout.branch,
                main: checkout.main,
                threads,
            });
        }
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
            "SELECT id FROM threads
             WHERE id IS NOT ?2 AND (
                 worktree = ?1 OR cwd = ?1 OR substr(cwd, 1, length(?1) + 1) = ?1 || '/'
             ) LIMIT 1",
            [path.into(), except.into()],
        )
        .await?
        .map(|row| row.id(0))
        .transpose()?;
        Ok(placement::removable(
            path,
            worktree::managed(&self.worktrees, path).await,
            user,
        )?)
    }

    pub(super) async fn remove_worktree(
        self: &Arc<Self>,
        path: &str,
        force: bool,
    ) -> Result<(), Error> {
        let workspace = Arc::clone(self);
        let path = path.to_owned();
        self.jobs
            .spawn(async move {
                let mut db = workspace.db.lock().await;
                workspace.removable(&mut db, &path, None).await?;
                if workspace
                    .git_actions
                    .lock()
                    .await
                    .contains_key(Path::new(&path))
                {
                    return Err(viz_git::Error::Busy.into());
                }
                let result = worktree::remove(&workspace.worktrees, &path, force).await;
                drop(db);
                result?;
                workspace.changed(None);
                Ok(())
            })
            .await
            .map_err(viz_git::Error::from)?
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
            return Ok(moved(placement.checkout).await);
        }
        let result = self.rebind(thread, binding, placement.clone()).await;
        if result.is_err() {
            worktree::discard(&self.worktrees, Path::new(&project), &placement).await;
        }
        result?;
        self.relist(thread);
        Ok(moved(placement.checkout).await)
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
        let handoff = threads::handoff(prompted, current.session.handoff, || Handoff::Moved {
            from: current.checkout.cwd,
        });
        exec(
            &mut tx,
            "UPDATE threads SET cwd = ?2, worktree = ?3, setup_failed = 0,
                session = json_set(session, '$.session_id', NULL, '$.handoff', json(?4))
             WHERE id = ?1",
            [
                thread.into(),
                placement.checkout.cwd.as_str().into(),
                placement.checkout.worktree.as_deref().into(),
                store::json(&handoff)?.into(),
            ],
        )
        .await?;
        tx.commit().await?;
        drop(db);
        Ok(())
    }
}

async fn moved(checkout: Checkout) -> Moved {
    let worktree = store::thread_worktree(checkout.worktree).await;
    Moved {
        worktree,
        cwd: checkout.cwd,
    }
}
