//! Thread lifetime and thread-scoped transactions. Harness and Git effects
//! run on either side of the commit as required by their ownership.

use std::{path::Path, sync::Arc, time::Duration};

use viz_core::{
    placement, runs as decisions,
    threads::{self, Patch},
    titles,
};
use viz_protocol::{
    LaunchThread, Launched, RunId, SendInput, SendMode, Sent, ThreadId, ThreadUpdated,
};

use super::{
    Workspace,
    admission::selection,
    attachments,
    rpc::UpdateMetadata,
    runs::{self, NewRun},
    sql::{Connection, exec, row},
    store::{self, NewThread, SavedSession},
    worker::Intent,
    worktree::{self, Placement},
};
use crate::{error::Error, tasks};

impl Workspace {
    /// Launches a thread; a worktree target is resolved, and a new worktree
    /// created, before the thread is recorded, and removed again if the
    /// launch fails or repeats an earlier request.
    pub(super) async fn launch(self: &Arc<Self>, args: LaunchThread) -> Result<Launched, Error> {
        threads::validate_request_id(args.request_id.as_deref())?;
        let Some(target) = args.worktree.clone() else {
            return self
                .launch_in(args, None)
                .await
                .map(|(launched, _)| launched);
        };
        let project = args
            .project
            .clone()
            .ok_or(placement::Rejection::NoProject)?;
        {
            let mut db = self.db.lock().await;
            if let Some(found) = launched(&mut db, args.request_id.as_deref()).await? {
                return Ok(found);
            }
            store::project(&mut db, &project).await?;
            drop(db);
        }
        let title = args
            .title
            .clone()
            .unwrap_or_else(|| titles::from_prompt(args.text.as_deref().unwrap_or_default()));
        let placement = self
            .place(&project, Path::new(&project), &target, &title)
            .await?;
        let result = self.launch_in(args, Some(&placement)).await;
        if !matches!(result, Ok((_, true))) {
            worktree::discard(Path::new(&project), &placement).await;
        }
        result.map(|(launched, _)| launched)
    }

    /// Records a launched thread, or returns the thread an earlier request
    /// with the same ID launched (with `false`).
    async fn launch_in(
        self: &Arc<Self>,
        args: LaunchThread,
        placement: Option<&Placement>,
    ) -> Result<(Launched, bool), Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(found) = launched(&mut tx, args.request_id.as_deref()).await? {
            return Ok((found, false));
        }
        threads::validate_title(args.title.as_deref())?;
        let (harness, _) = self.harness(args.harness.as_deref())?;
        let checkout = match placement {
            Some(placement) => placement.checkout.clone(),
            None => self.checkout_for(&mut tx, args.project.as_deref()).await?,
        };
        let attachments = attachments::decode(args.attachments)?;
        let session = SavedSession {
            desired: args.options,
            selection: selection(args.model, args.thinking),
            ..SavedSession::default()
        };
        let thread = store::create_thread(
            &mut tx,
            &NewThread {
                harness,
                checkout: &checkout,
                project: args.project.as_deref(),
                parent: None,
                parent_environment: None,
                title: args.title.as_deref(),
                session,
            },
        )
        .await?;
        let run = if args.text.is_some() || !attachments.is_empty() {
            let attachments = attachments::save(&mut tx, thread, attachments).await?;
            Some(
                runs::enqueue(
                    &mut tx,
                    thread,
                    NewRun {
                        attachments: &attachments,
                        ..NewRun::new(args.text.as_deref().unwrap_or_default())
                    },
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(request_id) = args.request_id {
            exec(
                &mut tx,
                "INSERT INTO launch_requests (request_id, thread, run) VALUES (?1, ?2, ?3)",
                [request_id.into(), thread.into(), run.into()],
            )
            .await?;
        }
        tx.commit().await?;
        self.kick(thread, run.is_none());
        drop(db);
        self.relist(thread);
        Ok((Launched { thread, run }, true))
    }

    pub(crate) async fn send(self: &Arc<Self>, args: SendInput) -> Result<Sent, Error> {
        let thread = args.thread;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        threads::validate_request_id(args.request_id.as_deref())?;
        let previous = match &args.request_id {
            Some(request_id) => runs::by_request(&mut tx, thread, request_id).await?,
            None => None,
        };
        let state = decisions::SubmissionState {
            previous,
            active: if args.mode == Some(SendMode::Steer) && previous.is_none() {
                runs::active(&mut tx, thread).await?
            } else {
                None
            },
        };
        let (place, steer) = match decisions::submit(state, args.mode) {
            decisions::Submission::Replay(run) => return Ok(Sent { run }),
            decisions::Submission::Enqueue { place, steer } => (place, steer),
        };
        let attachments =
            attachments::save(&mut tx, thread, attachments::decode(args.attachments)?).await?;
        let run = runs::enqueue(
            &mut tx,
            thread,
            NewRun {
                origin: args.origin.as_ref(),
                attachments: &attachments,
                request_id: args.request_id.as_deref(),
                place,
                ..NewRun::new(&args.text)
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(active) = steer {
            self.steer_active(thread, active);
        }
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok(Sent { run })
    }

    pub(super) fn steer_active(&self, thread: ThreadId, active: RunId) {
        if let Some(driver) = self.workers.get(thread) {
            driver.signal(active, Intent::Steer);
        }
    }

    pub(crate) async fn stop(
        &self,
        thread: ThreadId,
        requested: Option<RunId>,
    ) -> Result<(), Error> {
        let active = {
            let mut db = self.db.lock().await;
            store::require(&mut db, thread).await?;
            if let Some(run) = requested
                && runs::status(&mut db, thread, run).await?.is_none()
            {
                return Err(Error::NoRun { thread, run });
            }
            let mut tx = db.transaction().await?;
            let active = runs::cancel(&mut tx, thread, requested).await?;
            tx.commit().await?;
            if let Some(active) = active
                && let Some(driver) = self.workers.get(thread)
            {
                driver.signal(active, Intent::Stop);
            }
            drop(db);
            active
        };
        self.settled(thread);
        if let Some(active) = active {
            let result = self
                .wait_run(thread, Some(active), Duration::from_secs(120))
                .await?;
            if result.timed_out {
                return Err(Error::unavailable("ACP cancellation is still in progress"));
            }
        }
        Ok(())
    }

    pub(super) async fn update(&self, args: UpdateMetadata) -> Result<ThreadUpdated, Error> {
        let UpdateMetadata { thread, changes } = args;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let (current, in_worktree) = store::metadata(&mut tx, thread).await?;
        if let Patch::Set(project) = &changes.project {
            store::project(&mut tx, project).await?;
        }
        let updated = threads::update(current.clone(), in_worktree, changes)?;
        store::save_metadata(&mut tx, thread, &updated).await?;
        tx.commit().await?;
        drop(db);
        self.relist(thread);
        Ok(store::updated(&current, &updated))
    }

    /// Deletes a thread, and with `remove_worktree` the clean worktree viz
    /// created for it once its harness has stopped.
    pub(super) async fn delete(
        &self,
        thread: ThreadId,
        remove_worktree: bool,
    ) -> Result<(), Error> {
        let checkout = if remove_worktree {
            let mut db = self.db.lock().await;
            let binding = store::binding(&mut db, thread).await?;
            let path = binding
                .checkout
                .worktree
                .ok_or_else(|| Error::invalid("the thread runs in no worktree"))?;
            self.removable(&mut db, &path, Some(thread)).await?;
            drop(db);
            if !worktree::clean(Path::new(&path)).await? {
                return Err(Error::invalid(format!(
                    "worktree {path} has uncommitted changes or untracked files"
                )));
            }
            Some((binding.project.unwrap_or_else(|| path.clone()), path))
        } else {
            None
        };
        let driver = {
            let mut db = self.db.lock().await;
            let mut tx = db.transaction().await?;
            tasks::store::release(&mut tx, thread).await?;
            let count = exec(
                &mut tx,
                "DELETE FROM threads WHERE id = ?1",
                [thread.into()],
            )
            .await?;
            if count == 0 {
                return Err(Error::NoThread(thread));
            }
            tx.commit().await?;
            self.updates.remove(thread);
            let driver = self.workers.remove(thread);
            drop(db);
            driver
        };
        self.settled(thread);
        if let Some(driver) = driver {
            driver.close().await;
        }
        match tokio::fs::remove_dir_all(self.attachment_dir(thread)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%thread, %error, "removing materialized thread attachments");
            }
        }
        if let Some((source, path)) = checkout {
            worktree::remove(Path::new(&source), &path, false)
                .await
                .map_err(|error| {
                    Error::invalid(format!("deleted the thread, but not its worktree: {error}"))
                })?;
            self.changed(None);
        }
        Ok(())
    }
}

async fn launched(
    db: &mut Connection,
    request_id: Option<&str>,
) -> Result<Option<Launched>, Error> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    row(
        db,
        "SELECT thread, run FROM launch_requests WHERE request_id = ?1",
        [request_id.into()],
    )
    .await?
    .map(|row| {
        Ok(Launched {
            thread: row.id(0)?,
            run: row.opt_id(1)?,
        })
    })
    .transpose()
}
