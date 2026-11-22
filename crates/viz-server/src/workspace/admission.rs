//! Receiver-side admission and retraction. A receipt commits with its thread
//! and input. A tombstone prevents a delayed offer from starting work.

use std::{path::Path, sync::Arc};

use viz_core::{
    tasks::{self, Admission},
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
    store::{self, NewThread, SavedSession},
    worktree::{self, Placement},
};
use crate::{acp::Selection, error::Error};

impl Workspace {
    /// Resolve a worktree target before taking the write lease.
    pub(crate) async fn receive(self: &Arc<Self>, args: ReceiveTask) -> Result<Received, Error> {
        let Some(target) = args.target.worktree.clone() else {
            return self
                .receive_in(args, None)
                .await
                .map(|(received, _)| received);
        };
        if args.thread.is_some() {
            return Err(Error::invalid(
                "an existing task thread cannot change its worktree",
            ));
        }
        let (project, source) = {
            let mut db = self.db.lock().await;
            if let Some(found) =
                receipt(&mut db, (args.origin_environment.as_str(), &args.task)).await?
            {
                return Ok(found);
            }
            let parent = if args.origin_environment == self.environment {
                Some(store::binding(&mut db, args.parent).await?)
            } else {
                None
            };
            let project = args
                .target
                .project
                .clone()
                .or_else(|| parent.as_ref()?.project.clone())
                .ok_or_else(|| Error::invalid("a worktree requires a project"))?;
            store::project(&mut db, &project).await?;
            let source = match parent {
                Some(parent)
                    if args.target.project.is_none()
                        || parent.project.as_ref() == Some(&project) =>
                {
                    parent.cwd
                }
                _ => project.clone(),
            };
            drop(db);
            (project, source)
        };
        let placement = self
            .place(
                &project,
                Path::new(&source),
                &target,
                &titles::from_prompt(&args.text),
            )
            .await?;
        let result = self.receive_in(args, Some(&placement)).await;
        if !matches!(result, Ok((_, true))) {
            worktree::discard(Path::new(&project), &placement).await;
        }
        result.map(|(received, _)| received)
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
        match tasks::admit(found, retracted) {
            Admission::Replay(found) => {
                return Ok((found, false));
            }
            Admission::Cancelled => {
                return Err(Error::invalid("task was cancelled before admission"));
            }
            Admission::Create => {}
        }
        if args.text.is_empty() {
            return Err(Error::invalid("task text must not be empty"));
        }
        let decoded = attachments::decode(std::mem::take(&mut args.attachments))?;
        let thread = self.task_thread(&mut tx, &args, placement).await?;
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
        tx.commit().await?;
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok((Received { thread, run }, true))
    }

    /// The thread a task runs in. Without a placement or project, a child
    /// of a local parent runs in the parent's checkout.
    async fn task_thread(
        &self,
        db: &mut crate::workspace::sql::Connection,
        args: &ReceiveTask,
        placement: Option<&Placement>,
    ) -> Result<ThreadId, Error> {
        if let Some(thread) = args.thread {
            let binding = store::binding(db, thread).await?;
            if binding.parent != Some(args.parent)
                || binding.parent_environment.as_deref() != Some(args.origin_environment.as_str())
            {
                return Err(Error::invalid(
                    "target thread is not owned by this task parent",
                ));
            }
            if args
                .target
                .harness
                .as_ref()
                .is_some_and(|requested| *requested != binding.harness)
                || args
                    .target
                    .project
                    .as_ref()
                    .is_some_and(|requested| Some(requested) != binding.project.as_ref())
                || args.target.model.is_some()
                || args.target.thinking.is_some()
            {
                return Err(Error::invalid(
                    "existing task thread cannot change its harness, project, model, or thinking",
                ));
            }
            return Ok(thread);
        }
        let inherited = if args.origin_environment == self.environment {
            Some(store::binding(db, args.parent).await?)
        } else {
            None
        };
        let project = args
            .target
            .project
            .as_deref()
            .or_else(|| inherited.as_ref()?.project.as_deref());
        if placement.is_none() && args.target.worktree.is_some() {
            return Err(Error::invalid("unresolved worktree target"));
        }
        let selected = args
            .target
            .harness
            .as_deref()
            .or_else(|| inherited.as_ref().map(|parent| parent.harness.as_str()));
        let (harness, _) = self.harness(selected)?;
        let resolved;
        let (cwd, worktree) = if let Some(placement) = placement {
            (placement.cwd.as_str(), placement.worktree.as_deref())
        } else if args.target.project.is_none()
            && let Some(parent) = &inherited
        {
            (parent.cwd.as_str(), parent.worktree.as_deref())
        } else {
            resolved = self.cwd_for(db, args.target.project.as_deref()).await?;
            (store::path_text(&resolved)?, None)
        };
        let session = SavedSession {
            selection: selection(args.target.model.clone(), args.target.thinking.clone()),
            ..SavedSession::default()
        };
        store::create_thread(
            db,
            &NewThread {
                harness,
                cwd,
                project,
                worktree,
                parent: Some(args.parent),
                parent_environment: Some(&args.origin_environment),
                title: None,
                session,
            },
        )
        .await
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
