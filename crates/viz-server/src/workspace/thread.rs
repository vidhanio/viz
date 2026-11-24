//! Thread lifetime and thread-scoped transactions. Harness and Git effects
//! run on either side of the commit as required by their ownership.

use std::{path::Path, sync::Arc, time::Duration};

use viz_core::{
    placement, requests, runs as decisions,
    threads::{self, Patch},
    titles,
};
use viz_protocol::{
    LaunchThread, Launched, RunId, SendInput, SendMode, Sent, ThreadId, ThreadUpdated,
    UpdateThread, Withdrawal,
};

use super::{
    Workspace,
    admission::selection,
    attachments,
    runs::{self, NewRun},
    sql::{Connection, exec, row},
    store::{self, NewThread, SavedSession},
    worker::Intent,
    worktree::{self, Placement},
};
use crate::{acp::Housekeeping, error::Error, tasks};

impl Workspace {
    /// Launches a thread; a worktree target is resolved, and a new worktree
    /// created, before the thread is recorded, and removed again if the
    /// launch fails or repeats an earlier request. That work runs as a tracked
    /// job, so a caller that stops waiting cannot strand a new worktree.
    pub(super) async fn launch(
        self: &Arc<Self>,
        mut args: LaunchThread,
    ) -> Result<Launched, Error> {
        threads::validate_request_id(args.request_id.as_deref())?;
        if let Some(project) = &args.project {
            let (_, _, preferences) =
                store::preferences(&mut *self.db.lock().await, project).await?;
            viz_core::projects::launch_defaults(&mut args, preferences.settings);
        }
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
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move {
                let placement = workspace
                    .place(&project, Path::new(&project), &target, &title)
                    .await?;
                let result = workspace.launch_in(args, Some(&placement)).await;
                if !matches!(result, Ok((_, true))) {
                    worktree::discard(&workspace.worktrees, Path::new(&project), &placement).await;
                }
                result.map(|(launched, _)| launched)
            })
            .await
            .map_err(viz_git::Error::from)?
    }

    /// Records a launched thread, or returns the thread an earlier request
    /// with the same ID launched (with `false`).
    async fn launch_in(
        self: &Arc<Self>,
        args: LaunchThread,
        placement: Option<&Placement>,
    ) -> Result<(Launched, bool), Error> {
        let scratch = if args.project.is_none() && placement.is_none() {
            let title = args.title.as_deref().map_or_else(
                || titles::from_prompt(args.text.as_deref().unwrap_or_default()),
                str::to_owned,
            );
            Some(self.prepare_scratch(&title).await?)
        } else {
            None
        };
        let result = self.launch_tx(args, placement, scratch.as_ref()).await;
        if !matches!(result, Ok((_, true)))
            && let Some(scratch) = scratch
        {
            scratch.discard().await;
        }
        result
    }

    async fn launch_tx(
        self: &Arc<Self>,
        mut args: LaunchThread,
        placement: Option<&Placement>,
        scratch: Option<&super::scratch::PreparedScratch>,
    ) -> Result<(Launched, bool), Error> {
        let staged = attachments::stage(self, std::mem::take(&mut args.attachments)).await?;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        if let Some(found) = launched(&mut tx, args.request_id.as_deref()).await? {
            return Ok((found, false));
        }
        threads::validate_title(args.title.as_deref())?;
        let (harness, config) = self.harness(args.harness.as_deref())?;
        let access = if let Some(mode) = args.access {
            let options = super::sessions::catalog(&mut tx, harness)
                .await?
                .map_or_else(Vec::new, |catalog| {
                    catalog.options(config, args.model.as_deref())
                });
            Some(requests::choose(&options, &mode)?)
        } else {
            None
        };
        let checkout = match (placement, scratch) {
            (Some(placement), _) => placement.checkout.clone(),
            (None, Some(scratch)) => placement::Checkout {
                cwd: scratch.cwd.clone(),
                worktree: None,
            },
            (None, None) => self.checkout_for(&mut tx, args.project.as_deref()).await?,
        };
        let session = SavedSession {
            desired: args.options,
            selection: selection(args.model, args.thinking),
            ..SavedSession::default()
        };
        let thread = store::create_thread(
            self,
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
        if let Some(access) = &access {
            store::save_thread_access(&mut tx, thread, Some(access)).await?;
        }
        let run = if args.text.is_some() || !staged.is_empty() {
            let attachments = attachments::commit(&mut tx, thread, &staged, &[]).await?;
            Some(
                runs::enqueue(
                    &mut tx,
                    thread,
                    NewRun {
                        attachments: &attachments,
                        context: &args.context,
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
        drop(staged);
        self.kick(thread, run.is_none());
        drop(db);
        self.relist(thread);
        Ok((Launched { thread, run }, true))
    }

    pub(crate) async fn send(self: &Arc<Self>, mut args: SendInput) -> Result<Sent, Error> {
        let thread = args.thread;
        threads::validate_request_id(args.request_id.as_deref())?;
        let staged = attachments::stage(self, std::mem::take(&mut args.attachments)).await?;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
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
        let attachments = attachments::commit(&mut tx, thread, &staged, &args.stored).await?;
        let run = runs::enqueue(
            &mut tx,
            thread,
            NewRun {
                origin: args.origin.as_ref(),
                attachments: &attachments,
                context: &args.context,
                request_id: args.request_id.as_deref(),
                place,
                ..NewRun::new(&args.text)
            },
        )
        .await?;
        if let Some(active) = steer {
            store::withdraw_requests(&mut tx, Some(active), Withdrawal::Steered).await?;
        }
        tx.commit().await?;
        drop(staged);
        if let Some(active) = steer {
            self.steer_active(thread, active);
        }
        self.kick(thread, false);
        drop(db);
        self.relist(thread);
        Ok(Sent { run })
    }

    /// Call after committing the withdrawal of the run's pending requests.
    pub(super) fn steer_active(&self, thread: ThreadId, active: RunId) {
        self.requests.release_run(active);
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
            if let Some(active) = active {
                store::withdraw_requests(&mut tx, Some(active), Withdrawal::Cancelled).await?;
            }
            tx.commit().await?;
            if let Some(active) = active {
                self.requests.release_run(active);
                if let Some(driver) = self.workers.get(thread) {
                    driver.signal(active, Intent::Stop);
                }
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

    pub(super) async fn update(&self, args: UpdateThread) -> Result<ThreadUpdated, Error> {
        let UpdateThread {
            thread,
            title,
            project,
        } = args;
        let changes = threads::Changes { title, project };
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
        if let threads::Title::User(title) = &updated.title
            && updated.title != current.title
            && let Some(driver) = self.workers.get(thread)
        {
            driver.title(title.clone());
        }
        self.relist(thread);
        Ok(store::updated(&current, &updated))
    }

    /// Deletes a thread, and with `remove_worktree` the clean worktree viz
    /// created for it once its harness has stopped. Runs as a tracked job so
    /// a committed deletion always goes on to remove the worktree.
    pub(super) async fn delete(
        self: &Arc<Self>,
        thread: ThreadId,
        remove_worktree: bool,
    ) -> Result<(), Error> {
        let workspace = Arc::clone(self);
        self.jobs
            .spawn(async move { workspace.delete_now(thread, remove_worktree).await })
            .await
            .map_err(viz_git::Error::from)?
    }

    async fn delete_now(
        self: &Arc<Self>,
        thread: ThreadId,
        remove_worktree: bool,
    ) -> Result<(), Error> {
        let reservation = if remove_worktree {
            let path = {
                let mut db = self.db.lock().await;
                let binding = store::binding(&mut db, thread).await?;
                let path = binding
                    .checkout
                    .worktree
                    .ok_or_else(|| Error::invalid("the thread runs in no worktree"))?;
                drop(db);
                path
            };
            if !worktree::managed(&self.worktrees, &path).await {
                return Err(placement::Rejection::Unmanaged(path).into());
            }
            let (reservation, idle_driver) = {
                let mut db = self.db.lock().await;
                let binding = store::binding(&mut db, thread).await?;
                if binding.checkout.worktree.as_deref() != Some(path.as_str()) {
                    return Err(threads::Rejection::Moved.into());
                }
                runs::idle(&mut db, thread).await?;
                Self::require_unused(&mut db, &path, Some(thread)).await?;
                let reservation =
                    self.reserve(Path::new(&path), super::git::CheckoutAction::RemoveWorktree)?;
                let driver = self.workers.remove(thread);
                drop(db);
                (reservation, driver)
            };
            if let Some(driver) = idle_driver {
                driver.close().await;
            }
            worktree::remove(&self.worktrees, &path, false).await?;
            Some(reservation)
        } else {
            None
        };
        let (driver, binding) = {
            let mut db = self.db.lock().await;
            let mut tx = db.transaction().await?;
            let binding = store::binding(&mut tx, thread).await?;
            tasks::store::release(&mut tx, thread).await?;
            super::checkpoints::queue_releases(&mut tx, thread).await?;
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
            (driver, binding)
        };
        drop(reservation);
        self.settled(thread);
        self.due.notify_one();
        if let Some(driver) = driver {
            driver.close().await;
        }
        self.housekeep(&binding, Housekeeping::Delete);
        match tokio::fs::remove_dir_all(self.attachment_dir(thread)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%thread, %error, "removing materialized thread attachments");
            }
        }
        if remove_worktree {
            self.changed(None);
        }
        Ok(())
    }
}

pub(super) async fn launched(
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
