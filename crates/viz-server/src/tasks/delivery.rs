//! Interpret settled results, continuation outboxes, and durable stop requests.
//! The delivery lease serializes prompt wakes against family cancellation.

use std::{num::NonZeroU64, time::Duration};

use tokio::time;
use viz_core::tasks::{self as decisions, WakeMode};
use viz_protocol::{
    RunId,
    peer::{FollowUp, ProgressRequest, ProgressResult},
};

use super::{
    LOCAL_POLL, POLL, Tasks,
    store::{snapshot, stop_exists, suppress_descendant_wakes},
};
use crate::{error::Error, workspace::sql};

impl Tasks {
    pub(super) async fn deliver(&self, id: &str) -> Result<(), Error> {
        let _delivery = self.delivery.lock().await;
        let mut row = sql::row(&mut *self.workspace.db.lock().await,
            "SELECT parent, environment, thread, status, title, result, error, delivery FROM agent_tasks WHERE id = ?1 AND delivery != 'delivered'",
            [id.into()],
        ).await?.ok_or_else(|| Error::corrupt("task vanished before delivery"))?;
        let target = row.str(1)?;
        let location = match row.opt_u64(2)? {
            Some(child) => format!("{target} thread {child}"),
            None => target,
        };
        let status = row.str(3)?;
        let title = row.str(4)?;
        let details = row.opt_str(5)?.or(row.opt_str(6)?);
        let mut text = format!("Task {id} ({title}) on {location} {status}.");
        if let Some(details) = details {
            text.push_str("\n\n");
            text.push_str(&details);
        }
        let mode = match row.str(7)?.as_str() {
            "entry_only" => WakeMode::EntryOnly,
            "prompt" => WakeMode::Prompt,
            mode => {
                return Err(Error::corrupt(format!(
                    "invalid persisted task delivery {mode}"
                )));
            }
        };
        let wake = self.workspace.wake(row.u64(0)?, id, 0, &text, mode).await?;
        sql::exec(
            &mut *self.workspace.db.lock().await,
            "UPDATE agent_tasks SET delivery = 'delivered', wake_run = ?2 WHERE id = ?1",
            [id.into(), wake.into()],
        )
        .await?;
        self.signal();
        Ok(())
    }

    /// The child retains the outbox row until the parent acknowledges its
    /// durable wake. Retrying uses the same (task, continuation run) key.
    pub(super) async fn send_follow_up(
        &self,
        environment: &str,
        task: &str,
        run: u64,
    ) -> Result<(), Error> {
        let Some(mut row) = sql::row(
            &mut *self.workspace.db.lock().await,
            "SELECT thread, text FROM task_follow_ups WHERE environment = ?1 AND task = ?2 AND run = ?3",
            [environment.into(), task.into(), run.into()],
        ).await? else { return Ok(()) };
        let request = FollowUp {
            source: self.peers.local.clone(),
            task: task.to_owned(),
            thread: NonZeroU64::new(row.u64(0)?)
                .ok_or_else(|| Error::corrupt("missing child thread"))?,
            run: NonZeroU64::new(run).ok_or_else(|| Error::corrupt("missing continuation run"))?,
            text: row.str(1)?,
        };
        let acknowledged = if environment == self.peers.local {
            self.follow_up(request).await?
        } else {
            self.peers
                .call::<FollowUp>(environment, &request, Duration::ZERO)
                .await?
        };
        if acknowledged {
            sql::exec(
                &mut *self.workspace.db.lock().await,
                "DELETE FROM task_follow_ups WHERE environment = ?1 AND task = ?2 AND run = ?3",
                [environment.into(), task.into(), run.into()],
            )
            .await?;
        }
        Ok(())
    }

    /// Deliver the original result first. A stopped parent run can receive
    /// transcript entries, but cannot be woken by a follow-up.
    pub(crate) async fn follow_up(&self, request: FollowUp) -> Result<bool, Error> {
        let _delivery = self.delivery.lock().await;
        let (parent, mode, title) = {
            let mut db = self.workspace.db.lock().await;
            let Some(mut row) = sql::row(
                &mut db,
                "SELECT parent, status, delivery, wake_run, title, parent_run, thread FROM agent_tasks
                 WHERE id = ?1 AND environment = ?2",
                [request.task.as_str().into(), request.source.as_str().into()],
            )
            .await?
            else {
                return Ok(true);
            };
            let parent = row.u64(0)?;
            if row.str(2)? != "delivered" {
                return Ok(false);
            }
            if row.str(1)? != "completed" {
                return Ok(true);
            }
            if row.opt_u64(6)? != Some(request.thread.get()) {
                return Err(Error::invalid(
                    "task follow-up child does not match its receipt",
                ));
            }
            let stopped = if let Some(run) = row.opt_u64(5)? {
                stop_exists(&mut db, parent, run).await?
            } else {
                false
            };
            let mode = decisions::follow_up_mode(row.opt_u64(3)?.map(RunId), stopped);
            (parent, mode, row.str(4)?)
        };
        let text = format!(
            "Task {} ({title}) on {} thread {} continued.\n\n{}",
            request.task, request.source, request.thread, request.text,
        );
        self.workspace
            .wake(parent, &request.task, request.run.get(), &text, mode)
            .await?;
        self.signal();
        Ok(true)
    }

    pub(crate) async fn progress(&self, request: ProgressRequest) -> Result<ProgressResult, Error> {
        let ProgressRequest {
            thread,
            run,
            cancel,
        } = request;
        if thread.0 == 0 || run.0 == 0 {
            return Err(Error::invalid("invalid thread or run ID"));
        }
        let _delivery = self.delivery.lock().await;
        let mut db = self.workspace.db.lock().await;
        let family = snapshot(&mut db, thread.0, run.0).await?;
        let mut stopped = stop_exists(&mut db, thread.0, run.0).await?;
        if family.should_record_stop(cancel, stopped) {
            let mut tx = db.transaction().await?;
            sql::exec(
                &mut tx,
                "INSERT INTO stopped_task_runs (thread, run, handled) VALUES (?1, ?2, 0)",
                [thread.0.into(), run.0.into()],
            )
            .await?;
            suppress_descendant_wakes(&mut tx, thread.0, run.0).await?;
            tx.commit().await?;
            stopped = true;
            self.scan.notify_one();
            self.signal();
        }
        drop(db);
        Ok(ProgressResult {
            run: family.progress(thread, run, stopped),
        })
    }

    pub(super) async fn drive_stop(&self, thread: u64, run: u64) {
        loop {
            match self.stop_family(thread, run).await {
                Ok(true) => return,
                Ok(false) => time::sleep(LOCAL_POLL).await,
                Err(error) => {
                    tracing::error!(thread, run, %error, "cancelling delegated run tree");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    async fn stop_family(&self, thread: u64, run: u64) -> Result<bool, Error> {
        let active = {
            let _delivery = self.delivery.lock().await;
            let mut db = self.workspace.db.lock().await;
            let family = snapshot(&mut db, thread, run).await?;
            if !family.pending() {
                sql::exec(
                    &mut db,
                    "UPDATE stopped_task_runs SET handled = 1 WHERE thread = ?1 AND run = ?2",
                    [thread.into(), run.into()],
                )
                .await?;
                return Ok(true);
            }
            let mut tx = db.transaction().await?;
            suppress_descendant_wakes(&mut tx, thread, run).await?;
            tx.commit().await?;
            drop(db);
            family.active
        };
        for active_run in active {
            if let Err(error) = self.workspace.stop(thread, Some(active_run.0)).await
                && !error.rejected()
            {
                return Err(error);
            }
        }
        Ok(false)
    }
}
