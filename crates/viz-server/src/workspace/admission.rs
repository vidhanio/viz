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
    sql::{Connection, exec, row},
    store::{self, Binding, NewThread, SavedSession},
    worktree::{self, Placement},
};
use crate::{acp::Selection, error::Error};

impl Workspace {
    /// Resolve a worktree target before taking the write lease, in a tracked
    /// job so a new worktree is either recorded or discarded.
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
        let parent = {
            let mut db = self.db.lock().await;
            if let Some(found) =
                receipt(&mut db, (args.origin_environment.as_str(), &args.task)).await?
            {
                return Ok(found);
            }
            let parent = self.parent(&mut db, &args).await?;
            drop(db);
            parent
        };
        let (project, source) = placement::child_worktree(
            args.target.project.as_deref(),
            parent.as_ref().map(Parent::placed),
        )?;
        store::project(&mut *self.db.lock().await, project).await?;
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
        mut args: ReceiveTask,
        placement: Option<&Placement>,
    ) -> Result<(Received, bool), Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let key = (args.origin_environment.as_str(), &args.task);
        let found = receipt(&mut tx, key).await?;
        let retracted = if found.is_some() {
            false
        } else {
            row(
                &mut tx,
                "SELECT 1 FROM task_retractions WHERE environment = ?1 AND task = ?2",
                [key.0.into(), key.1.into()],
            )
            .await?
            .is_some()
        };
        match tasks::admit(found, retracted)? {
            Admission::Replay(found) => return Ok((found, false)),
            Admission::Create => {}
        }
        if args.text.is_empty() {
            return Err(Rejection::EmptyText.into());
        }
        let decoded = attachments::decode(std::mem::take(&mut args.attachments))?;
        let (thread, scratch) = self.task_thread(&mut tx, &args, placement).await?;
        let attachments = attachments::save(&mut tx, thread, decoded).await?;
        let run = runs::enqueue(
            &mut tx,
            thread,
            NewRun {
                attachments: &attachments,
                ..NewRun::new(&args.text)
            },
        )
        .await?;
        exec(
            &mut tx,
            "INSERT INTO task_receipts (environment, task, thread, run) VALUES (?1, ?2, ?3, ?4)",
            [
                args.origin_environment.into(),
                args.task.into(),
                thread.into(),
                run.into(),
            ],
        )
        .await?;
        let scratch_title = scratch.then(|| {
            args.title
                .unwrap_or_else(|| titles::from_prompt(&args.text))
        });
        self.commit_thread(tx, thread, scratch_title.as_deref())
            .await?;
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok((Received { thread, run }, true))
    }

    /// The local thread an offer came from.
    async fn parent(
        &self,
        db: &mut Connection,
        offer: &ReceiveTask,
    ) -> Result<Option<Parent>, Error> {
        if offer.origin_environment != self.environment {
            return Ok(None);
        }
        let binding = store::binding(db, offer.parent).await?;
        let within = self.within(&binding).await;
        Ok(Some(Parent { binding, within }))
    }

    /// The thread a task runs in: the child it names, or a new one in its
    /// resolved worktree or wherever [`placement::child`] places it.
    async fn task_thread(
        &self,
        db: &mut Connection,
        args: &ReceiveTask,
        resolved: Option<&Placement>,
    ) -> Result<(ThreadId, bool), Error> {
        if let Some(thread) = args.thread {
            let binding = store::binding(db, thread).await?;
            tasks::reuse(Reuse {
                offer: args,
                parent: binding.parent,
                parent_environment: binding.parent_environment.as_deref(),
                harness: &binding.harness,
                project: binding.project.as_deref(),
            })?;
            return Ok((thread, false));
        }
        let parent = self.parent(db, args).await?;
        let placed = parent.as_ref().map(Parent::placed);
        let target = args.target.project.as_deref();
        let project = placement::child_project(target, placed);
        if resolved.is_none() && args.target.worktree.is_some() {
            return Err(Error::invalid("unresolved worktree target"));
        }
        let selected = args.target.harness.as_deref().or_else(|| {
            parent
                .as_ref()
                .map(|parent| parent.binding.harness.as_str())
        });
        let (harness, _) = self.harness(selected)?;
        let checkout = match (resolved, placement::child(target, placed)) {
            (Some(resolved), _) => resolved.checkout.clone(),
            (None, Place::Parent(checkout)) => checkout.clone(),
            (None, Place::Project(project)) => self.checkout_for(db, Some(project)).await?,
            (None, Place::Default) => self.checkout_for(db, None).await?,
        };
        let session = SavedSession {
            selection: selection(args.target.model.clone(), args.target.thinking.clone()),
            ..SavedSession::default()
        };
        let thread = store::create_thread(
            self,
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
        .await?;
        Ok((thread, project.is_none()))
    }

    pub(crate) async fn retract(&self, args: RetractTask) -> Result<Option<Received>, Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(found) = receipt(&mut tx, (&args.origin_environment, &args.task)).await? {
            return Ok(Some(found));
        }
        exec(&mut tx, "INSERT INTO task_retractions (environment, task) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            [args.origin_environment.into(), args.task.into()]).await?;
        tx.commit().await?;
        drop(db);
        Ok(None)
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

async fn receipt(
    tx: &mut Connection,
    (environment, task): (&str, &TaskId),
) -> Result<Option<Received>, Error> {
    row(
        tx,
        "SELECT thread, run FROM task_receipts WHERE environment = ?1 AND task = ?2",
        [environment.into(), task.into()],
    )
    .await?
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
