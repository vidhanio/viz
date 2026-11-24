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
    Workspace,
    git::CheckoutAction,
    runs,
    sql::{Conn, exec, row, rows},
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
                let project_owned = project.to_owned();
                let (_, _, preferences) = self
                    .db
                    .read(move |conn| store::preferences(conn, &project_owned))
                    .await?;
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
        let project_owned = project.to_owned();
        let project = self
            .db
            .read(move |conn| store::require_project(conn, &project_owned))
            .await?;
        let listed = worktree::list(Path::new(&project)).await?;
        let project_for_query = project.clone();
        let filed: Vec<(ThreadId, String)> = self
            .db
            .read(move |conn| {
                rows(
                    conn,
                    "SELECT id, cwd FROM threads WHERE project = ?1 OR worktree IS NOT NULL ORDER BY id",
                    [project_for_query.as_str().into()],
                )?
                .map(|mut row| Ok((row.id(0)?, row.str(1)?)))
                .collect()
            })
            .await?;
        let mut by_checkout = std::collections::HashMap::<String, Vec<ThreadId>>::new();
        let mut resolved = std::collections::HashMap::new();
        for (thread, cwd) in filed {
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
                by_checkout.entry(path.clone()).or_default().push(thread);
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

    /// Checks that no thread other than `except` runs in `path`.
    pub(super) fn require_unused(
        db: &Conn,
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
        )?
        .map(|row| row.id(0))
        .transpose()?;
        Ok(placement::removable(path, true, user)?)
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
                if !worktree::managed(&workspace.worktrees, &path).await {
                    return Err(placement::Rejection::Unmanaged(path).into());
                }
                let path_for_write = path.clone();
                let ws = Arc::clone(&workspace);
                let reservation = workspace
                    .db
                    .write(move |conn| {
                        Self::require_unused(conn, &path_for_write, None)?;
                        ws.reserve(Path::new(&path_for_write), CheckoutAction::RemoveWorktree)
                    })
                    .await?;
                let result = worktree::remove(&workspace.worktrees, &path, force).await;
                drop(reservation);
                result?;
                workspace.changed(None);
                Ok(())
            })
            .await
            .map_err(viz_git::Error::from)?
    }

    /// Moves an idle thread. Its harness stops; the next run starts a new
    /// session in the new directory (see [`store::SavedSession::handoff`]).
    /// Placement and rebinding run as a tracked job so a new worktree is
    /// either bound or discarded even if the caller stops waiting.
    pub(super) async fn move_thread(self: &Arc<Self>, args: MoveThread) -> Result<Moved, Error> {
        let thread = args.thread;
        let (binding, title) = self
            .db
            .read_snapshot(move |tx| {
                let binding = store::binding(tx, thread)?;
                runs::idle(tx, thread)?;
                let title = store::thread_title(tx, thread)?;
                Ok((binding, title))
            })
            .await?;
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
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move {
                let placement = workspace
                    .place(&project, Path::new(&source), &args.worktree, &title)
                    .await?;
                if placement.checkout == binding.checkout {
                    return Ok(moved(placement.checkout).await);
                }
                let result = workspace.rebind(thread, binding, placement.clone()).await;
                if result.is_err() {
                    worktree::discard(&workspace.worktrees, Path::new(&project), &placement).await;
                }
                result?;
                workspace.relist(thread);
                Ok(moved(placement.checkout).await)
            })
            .await
            .map_err(viz_git::Error::from)?
    }

    async fn rebind(
        self: &Arc<Self>,
        thread: ThreadId,
        before: store::Binding,
        placement: Placement,
    ) -> Result<(), Error> {
        let ws = Arc::clone(self);
        let driver = self
            .db
            .write(move |conn| {
                store::require(conn, thread)?;
                ws.driver(thread)
            })
            .await?;
        driver.rebind(before, placement).await
    }

    /// Only the session owner may change the binding while the thread is idle.
    pub(super) async fn apply_checkout(
        &self,
        thread: ThreadId,
        before: &store::Binding,
        placement: &Placement,
    ) -> Result<(), Error> {
        let dest = Path::new(
            placement
                .checkout
                .worktree
                .as_deref()
                .unwrap_or(&placement.checkout.cwd),
        );
        let canonical = tokio::fs::canonicalize(dest).await?;
        let expected_checkout = before.checkout.clone();
        let next_checkout = placement.checkout.clone();
        let reservations = Arc::clone(&self.reservations);
        self.db
            .write_tx(move |tx| {
                let current = store::binding(tx, thread)?;
                if current.checkout != expected_checkout {
                    return Err(threads::Rejection::Moved.into());
                }
                runs::idle(tx, thread)?;
                if reservations.removing(&canonical) {
                    return Err(viz_git::Error::Busy.into());
                }
                reservations.admit_checkout(&next_checkout)?;
                let prompted = row(
                    tx,
                    "SELECT 1 FROM runs WHERE thread = ?1 LIMIT 1",
                    [thread.into()],
                )?
                .is_some();
                let handoff =
                    threads::handoff(prompted, current.session.handoff, || Handoff::Moved {
                        from: current.checkout.cwd,
                    });
                exec(
                    tx,
                    "UPDATE threads SET cwd = ?2, worktree = ?3, setup_failed = 0,
                        session = json_set(session, '$.session_id', NULL, '$.handoff', json(?4))
                     WHERE id = ?1",
                    [
                        thread.into(),
                        next_checkout.cwd.as_str().into(),
                        next_checkout.worktree.as_deref().into(),
                        store::json(&handoff)?.into(),
                    ],
                )?;
                Ok(())
            })
            .await
    }
}

async fn moved(checkout: Checkout) -> Moved {
    let worktree = store::thread_worktree(checkout.worktree).await;
    Moved {
        worktree,
        cwd: checkout.cwd,
    }
}
