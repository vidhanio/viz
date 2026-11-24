//! Receiver-side admission and retraction. A receipt commits with its thread
//! and input. A tombstone prevents a delayed offer from starting work.

use std::{path::Path, sync::Arc};

use viz_core::{
    placement::{self, Place, Placed},
    tasks::{self, Admission, Rejection, Reuse},
    titles,
};
use viz_protocol::{
    TaskId, ThreadId,
    peer::{ReceiveTask, Received, RetractTask},
};

use super::{
    Workspace, attachments,
    runs::{self, NewRun},
    sql::{self, Conn, exec, row},
    store::{self, Binding, NewThread, SavedSession},
    worktree::{self, Placement},
};
use crate::{acp::Selection, error::Error};

impl Workspace {
    /// Resolve a worktree target before taking the write transaction, in a
    /// tracked job so a new worktree is either recorded or discarded.
    pub(crate) async fn receive(self: &Arc<Self>, args: ReceiveTask) -> Result<Received, Error> {
        let Some(target) = args.target.worktree.clone() else {
            return self
                .receive_in(args, None)
                .await
                .map(|(received, _)| received);
        };
        if args.thread.is_some() {
            return Err(Rejection::MovedThread.into());
        }
        let origin_env = args.origin_environment.clone();
        let task_id = args.task.clone();
        let parent_id = args.parent;
        let local_env = self.environment.clone();
        let precheck = self
            .db
            .read_snapshot(move |tx| {
                if let Some(found) = receipt(tx, (&origin_env, &task_id))? {
                    return Ok(Err(found));
                }
                let binding = if origin_env == local_env {
                    Some(store::binding(tx, parent_id)?)
                } else {
                    None
                };
                Ok(Ok(binding))
            })
            .await?;
        let parent_binding = match precheck {
            Ok(binding) => binding,
            Err(found) => return Ok(found),
        };
        let parent = self.resolve_parent(parent_binding).await;
        let (project, source) = placement::child_worktree(
            args.target.project.as_deref(),
            parent.as_ref().map(Parent::placed),
        )?;
        let project_for_read = project.to_owned();
        self.db
            .read(move |conn| store::require_project(conn, &project_for_read))
            .await?;
        let (project, source) = (project.to_owned(), source.to_owned());
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move {
                let placement = workspace
                    .place(
                        &project,
                        Path::new(&source),
                        &target,
                        &titles::from_prompt(&args.text),
                    )
                    .await?;
                let result = workspace.receive_in(args, Some(&placement)).await;
                if !matches!(result, Ok((_, true))) {
                    worktree::discard(&workspace.worktrees, Path::new(&project), &placement).await;
                }
                result.map(|(received, _)| received)
            })
            .await
            .map_err(viz_git::Error::from)?
    }

    async fn receive_in(
        self: &Arc<Self>,
        args: ReceiveTask,
        placement: Option<&Placement>,
    ) -> Result<(Received, bool), Error> {
        let parent_binding = if args.thread.is_none() {
            let origin_env = args.origin_environment.clone();
            let task_id = args.task.clone();
            let parent_id = args.parent;
            let local_env = self.environment.clone();
            let precheck = self
                .db
                .read_snapshot(move |tx| {
                    if let Some(found) = receipt(tx, (&origin_env, &task_id))? {
                        return Ok(Err(found));
                    }
                    let binding = if origin_env == local_env {
                        Some(store::binding(tx, parent_id)?)
                    } else {
                        None
                    };
                    Ok(Ok(binding))
                })
                .await?;
            match precheck {
                Ok(binding) => binding,
                Err(found) => return Ok((found, false)),
            }
        } else {
            None
        };
        let parent = self.resolve_parent(parent_binding).await;
        let needs_scratch = args.thread.is_none()
            && placement.is_none()
            && placement::child_project(
                args.target.project.as_deref(),
                parent.as_ref().map(Parent::placed),
            )
            .is_none();
        let scratch = if needs_scratch {
            let title = args
                .title
                .as_deref()
                .map_or_else(|| titles::from_prompt(&args.text), str::to_owned);
            Some(self.prepare_scratch(&title).await?)
        } else {
            None
        };
        let result = self
            .receive_tx(args, placement, parent, scratch.as_ref())
            .await;
        if !matches!(result, Ok((_, true)))
            && let Some(scratch) = scratch
        {
            scratch.discard().await;
        }
        result
    }

    async fn receive_tx(
        self: &Arc<Self>,
        args: ReceiveTask,
        placement: Option<&Placement>,
        parent: Option<Parent>,
        scratch: Option<&super::scratch::PreparedScratch>,
    ) -> Result<(Received, bool), Error> {
        let placement_checkout = placement.map(|p| p.checkout.clone());
        let scratch_cwd = scratch.map(|s| s.cwd.clone());
        let ws = Arc::clone(self);
        let (received, created) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                let key = (args.origin_environment.as_str(), &args.task);
                let found = receipt(&tx, key)?;
                let retracted = if found.is_some() {
                    false
                } else {
                    row(
                        &tx,
                        "SELECT 1 FROM task_retractions WHERE environment = ?1 AND task = ?2",
                        [key.0.into(), key.1.into()],
                    )?
                    .is_some()
                };
                match tasks::admit(found, retracted)? {
                    Admission::Replay(found) => return Ok((found, false)),
                    Admission::Create => {}
                }
                if args.text.is_empty() {
                    return Err(Rejection::EmptyText.into());
                }
                let thread = ws.task_thread(
                    &tx,
                    &args,
                    placement_checkout.as_ref(),
                    parent.as_ref(),
                    scratch_cwd.as_deref(),
                )?;
                let attachments = attachments::link_peer(&tx, thread, &args.attachments)?;
                let run = runs::enqueue(
                    &tx,
                    thread,
                    NewRun {
                        attachments: &attachments,
                        ..NewRun::new(&args.text)
                    },
                )?;
                exec(
                    &tx,
                    "INSERT INTO task_receipts (environment, task, thread, run) VALUES (?1, ?2, ?3, ?4)",
                    [
                        args.origin_environment.into(),
                        args.task.into(),
                        thread.into(),
                        run.into(),
                    ],
                )?;
                tx.commit()?;
                ws.kick(thread, false);
                Ok((Received { thread, run }, true))
            })
            .await?;
        if created {
            self.relist(received.thread);
        }
        Ok((received, created))
    }

    /// Resolves whether the local parent sits within its project repository
    /// outside any SQLite closure.
    async fn resolve_parent(&self, binding: Option<Binding>) -> Option<Parent> {
        let binding = binding?;
        let within = self.within(&binding).await;
        Some(Parent { binding, within })
    }

    /// The thread a task runs in: the child it names, or a new one in its
    /// resolved worktree or wherever [`placement::child`] places it.
    fn task_thread(
        &self,
        db: &Conn,
        args: &ReceiveTask,
        resolved: Option<&placement::Checkout>,
        parent: Option<&Parent>,
        scratch_cwd: Option<&str>,
    ) -> Result<ThreadId, Error> {
        if let Some(thread) = args.thread {
            let binding = store::binding(db, thread)?;
            tasks::reuse(Reuse {
                offer: args,
                parent: binding.parent,
                parent_environment: binding.parent_environment.as_deref(),
                harness: &binding.harness,
                project: binding.project.as_deref(),
            })?;
            return Ok(thread);
        }
        let placed = parent.map(Parent::placed);
        let target = args.target.project.as_deref();
        let project = placement::child_project(target, placed);
        if resolved.is_none() && args.target.worktree.is_some() {
            return Err(Error::invalid("unresolved worktree target"));
        }
        let selected = args
            .target
            .harness
            .as_deref()
            .or_else(|| parent.map(|parent| parent.binding.harness.as_str()));
        let (harness, _) = self.harness(selected)?;
        let checkout = match (
            resolved,
            project,
            scratch_cwd,
            placement::child(target, placed),
        ) {
            (Some(resolved), _, _, _) => resolved.clone(),
            (None, None, Some(cwd), _) => placement::Checkout {
                cwd: cwd.to_owned(),
                worktree: None,
            },
            (None, _, _, Place::Parent(checkout)) => checkout.clone(),
            (None, _, _, Place::Project(project)) => self.checkout_for(db, Some(project))?,
            (None, _, _, Place::Default) => self.checkout_for(db, None)?,
        };
        let session = SavedSession {
            selection: selection(args.target.model.clone(), args.target.thinking.clone()),
            ..SavedSession::default()
        };
        store::create_thread(
            &self.reservations,
            db,
            &NewThread {
                harness,
                checkout: &checkout,
                project,
                parent: Some(args.parent),
                parent_environment: Some(&args.origin_environment),
                title: args.title.as_deref(),
                session,
            },
        )
    }

    pub(crate) async fn retract(&self, args: RetractTask) -> Result<Option<Received>, Error> {
        self.db
            .write_tx(move |tx| {
                if let Some(found) = receipt(tx, (&args.origin_environment, &args.task))? {
                    return Ok(Some(found));
                }
                exec(
                    tx,
                    "INSERT INTO task_retractions (environment, task) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
                    [args.origin_environment.into(), args.task.into()],
                )?;
                Ok(None)
            })
            .await
    }
}

/// A local parent as found on disk.
struct Parent {
    binding: Binding,
    within: bool,
}

impl Parent {
    fn placed(&self) -> Placed<'_> {
        Placed {
            project: self.binding.project.as_deref(),
            checkout: &self.binding.checkout,
            within: self.within,
        }
    }
}

fn receipt(tx: &Conn, (environment, task): (&str, &TaskId)) -> Result<Option<Received>, Error> {
    row(
        tx,
        "SELECT thread, run FROM task_receipts WHERE environment = ?1 AND task = ?2",
        [environment.into(), task.into()],
    )?
    .map(|row| {
        Ok(Received {
            thread: row.id(0)?,
            run: row.id(1)?,
        })
    })
    .transpose()
}

pub(super) fn selection(model: Option<String>, thinking: Option<String>) -> Option<Selection> {
    (model.is_some() || thinking.is_some()).then_some(Selection { model, thinking })
}
