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
    sql::{self, Conn, exec, row},
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
        principal_key: String,
        mut args: LaunchThread,
    ) -> Result<Launched, Error> {
        threads::validate_request_id(args.request_id.as_deref())?;
        if let Some(project) = args.project.clone() {
            let (_, _, preferences) = self
                .db
                .read(move |conn| store::preferences(conn, &project))
                .await?;
            viz_core::projects::launch_defaults(&mut args, preferences.settings);
        }
        let Some(target) = args.worktree.clone() else {
            return self
                .launch_in(principal_key, args, None)
                .await
                .map(|(launched, _)| launched);
        };
        let project = args
            .project
            .clone()
            .ok_or(placement::Rejection::NoProject)?;
        let request_id = args.request_id.clone();
        let project_for_read = project.clone();
        if let Some(found) = self
            .db
            .read_snapshot(move |tx| {
                if let Some(found) = launched(tx, request_id.as_deref())? {
                    return Ok(Some(found));
                }
                store::require_project(tx, &project_for_read)?;
                Ok(None)
            })
            .await?
        {
            return Ok(found);
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
                let result = workspace
                    .launch_in(principal_key, args, Some(&placement))
                    .await;
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
        principal_key: String,
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
        let result = self
            .launch_tx(principal_key, args, placement, scratch.as_ref())
            .await;
        if !matches!(result, Ok((_, true)))
            && let Some(scratch) = scratch
        {
            scratch.discard().await;
        }
        result
    }

    async fn launch_tx(
        self: &Arc<Self>,
        principal_key: String,
        args: LaunchThread,
        placement: Option<&Placement>,
        scratch: Option<&super::scratch::PreparedScratch>,
    ) -> Result<(Launched, bool), Error> {
        let placement_checkout = placement.map(|p| p.checkout.clone());
        let scratch_cwd = scratch.map(|s| s.cwd.clone());
        let ws = Arc::clone(self);
        let (launched, created) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                if let Some(found) = launched(&tx, args.request_id.as_deref())? {
                    return Ok((found, false));
                }
                threads::validate_title(args.title.as_deref())?;
                let (harness, config) = ws.harness(args.harness.as_deref())?;
                let access = if let Some(mode) = args.access {
                    let options = super::sessions::catalog(&tx, harness)?
                        .map_or_else(Vec::new, |catalog| {
                            catalog.options(config, args.model.as_deref())
                        });
                    Some(requests::choose(&options, &mode)?)
                } else {
                    None
                };
                let checkout = match (placement_checkout, scratch_cwd) {
                    (Some(checkout), _) => checkout,
                    (None, Some(cwd)) => placement::Checkout {
                        cwd,
                        worktree: None,
                    },
                    (None, None) => ws.checkout_for(&tx, args.project.as_deref())?,
                };
                let session = SavedSession {
                    desired: args.options,
                    selection: selection(args.model, args.thinking),
                    ..SavedSession::default()
                };
                let thread = store::create_thread(
                    &ws.reservations,
                    &tx,
                    &NewThread {
                        harness,
                        checkout: &checkout,
                        project: args.project.as_deref(),
                        parent: None,
                        parent_environment: None,
                        title: args.title.as_deref(),
                        session,
                    },
                )?;
                if let Some(access) = &access {
                    store::save_thread_access(&tx, thread, Some(access))?;
                }
                let run = if args.text.is_some() || !args.attachments.is_empty() {
                    let attachments = attachments::claim_staged(
                        &tx,
                        &ws.leases,
                        &principal_key,
                        thread,
                        &args.attachments,
                        &[],
                    )?;
                    Some(runs::enqueue(
                        &tx,
                        thread,
                        NewRun {
                            attachments: &attachments,
                            context: &args.context,
                            ..NewRun::new(args.text.as_deref().unwrap_or_default())
                        },
                    )?)
                } else {
                    None
                };
                if let Some(request_id) = args.request_id {
                    exec(
                        &tx,
                        "INSERT INTO launch_requests (request_id, thread, run) VALUES (?1, ?2, ?3)",
                        [request_id.into(), thread.into(), run.into()],
                    )?;
                }
                tx.commit()?;
                ws.kick(thread, run.is_none());
                Ok((Launched { thread, run }, true))
            })
            .await?;
        if created {
            self.relist(launched.thread);
        }
        Ok((launched, created))
    }

    pub(crate) async fn send(
        self: &Arc<Self>,
        principal_key: String,
        args: SendInput,
    ) -> Result<Sent, Error> {
        let thread = args.thread;
        threads::validate_request_id(args.request_id.as_deref())?;
        let ws = Arc::clone(self);
        let (sent, created) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                store::require(&tx, thread)?;
                let previous = match &args.request_id {
                    Some(request_id) => runs::by_request(&tx, thread, request_id)?,
                    None => None,
                };
                let state = decisions::SubmissionState {
                    previous,
                    active: if args.mode == Some(SendMode::Steer) && previous.is_none() {
                        runs::active(&tx, thread)?
                    } else {
                        None
                    },
                };
                let (place, steer) = match decisions::submit(state, args.mode) {
                    decisions::Submission::Replay(run) => return Ok((Sent { run }, false)),
                    decisions::Submission::Enqueue { place, steer } => (place, steer),
                };
                let attachments = attachments::claim_staged(
                    &tx,
                    &ws.leases,
                    &principal_key,
                    thread,
                    &args.attachments,
                    &args.stored,
                )?;
                let run = runs::enqueue(
                    &tx,
                    thread,
                    NewRun {
                        origin: args.origin.as_ref(),
                        attachments: &attachments,
                        context: &args.context,
                        request_id: args.request_id.as_deref(),
                        place,
                        ..NewRun::new(&args.text)
                    },
                )?;
                if let Some(active) = steer {
                    store::withdraw_requests(&tx, Some(active), Withdrawal::Steered)?;
                }
                tx.commit()?;
                if let Some(active) = steer {
                    ws.steer_active(thread, active);
                }
                ws.kick(thread, false);
                Ok((Sent { run }, true))
            })
            .await?;
        if created {
            self.relist(thread);
        }
        Ok(sent)
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
        let requests = Arc::clone(&self.requests);
        let workers = Arc::clone(&self.workers);
        let active = self
            .db
            .write(move |conn| {
                store::require(conn, thread)?;
                if let Some(run) = requested
                    && runs::status(conn, thread, run)?.is_none()
                {
                    return Err(Error::NoRun { thread, run });
                }
                let tx = sql::immediate(conn)?;
                let active = runs::cancel(&tx, thread, requested)?;
                if let Some(active) = active {
                    store::withdraw_requests(&tx, Some(active), Withdrawal::Cancelled)?;
                }
                tx.commit()?;
                if let Some(active) = active {
                    requests.release_run(active);
                    if let Some(driver) = workers.get(thread) {
                        driver.signal(active, Intent::Stop);
                    }
                }
                Ok(active)
            })
            .await?;
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
        let (current, updated) = self
            .db
            .write_tx(move |tx| {
                let (current, in_worktree) = store::metadata(tx, thread)?;
                if let Patch::Set(project) = &changes.project {
                    store::require_project(tx, project)?;
                }
                let updated = threads::update(current.clone(), in_worktree, changes)?;
                store::save_metadata(tx, thread, &updated)?;
                Ok((current, updated))
            })
            .await?;
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
            let path = self
                .db
                .read(move |conn| {
                    let binding = store::binding(conn, thread)?;
                    binding
                        .checkout
                        .worktree
                        .ok_or_else(|| Error::invalid("the thread runs in no worktree"))
                })
                .await?;
            if !worktree::managed(&self.worktrees, &path).await {
                return Err(placement::Rejection::Unmanaged(path).into());
            }
            let path_for_write = path.clone();
            let ws = Arc::clone(self);
            let (reservation, idle_driver) = self
                .db
                .write(move |conn| {
                    let binding = store::binding(conn, thread)?;
                    if binding.checkout.worktree.as_deref() != Some(path_for_write.as_str()) {
                        return Err(threads::Rejection::Moved.into());
                    }
                    runs::idle(conn, thread)?;
                    Self::require_unused(conn, &path_for_write, Some(thread))?;
                    let reservation = ws.reserve(
                        Path::new(&path_for_write),
                        super::git::CheckoutAction::RemoveWorktree,
                    )?;
                    let driver = ws.workers.remove(thread);
                    Ok((reservation, driver))
                })
                .await?;
            if let Some(driver) = idle_driver {
                driver.close().await;
            }
            worktree::remove(&self.worktrees, &path, false).await?;
            Some(reservation)
        } else {
            None
        };
        let updates = Arc::clone(&self.updates);
        let workers = Arc::clone(&self.workers);
        let (driver, binding) = self
            .db
            .write(move |conn| {
                let tx = sql::immediate(conn)?;
                let binding = store::binding(&tx, thread)?;
                tasks::store::release(&tx, thread)?;
                super::checkpoints::queue_releases(&tx, thread)?;
                let count = exec(&tx, "DELETE FROM threads WHERE id = ?1", [thread.into()])?;
                if count == 0 {
                    return Err(Error::NoThread(thread));
                }
                tx.commit()?;
                updates.remove(thread);
                let driver = workers.remove(thread);
                Ok((driver, binding))
            })
            .await?;
        drop(reservation);
        self.thread_removed(thread);
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
        if remove_worktree && let Some(project) = binding.project {
            self.project_changed(project);
        }
        Ok(())
    }
}

pub(super) fn launched(db: &Conn, request_id: Option<&str>) -> Result<Option<Launched>, Error> {
    let Some(request_id) = request_id else {
        return Ok(None);
    };
    row(
        db,
        "SELECT thread, run FROM launch_requests WHERE request_id = ?1",
        [request_id.into()],
    )?
    .map(|row| {
        Ok(Launched {
            thread: row.id(0)?,
            run: row.opt_id(1)?,
        })
    })
    .transpose()
}
