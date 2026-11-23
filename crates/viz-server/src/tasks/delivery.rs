//! Interpret settled results, continuation outboxes, and durable stop requests.
//! The delivery lease serializes prompt wakes against family cancellation.

use std::time::Duration;

use tokio::time;
use viz_core::tasks::{self as decisions, Command, Step, WakeSource};
use viz_protocol::{
    RunId, TaskId, ThreadId,
    peer::{FollowUp, ProgressRequest, ProgressResult},
};

use super::{
    LOCAL_POLL, POLL, Tasks,
    store::{self, snapshot, stop_exists, suppress},
};
use crate::{error::Error, workspace::sql};

impl Tasks {
    /// Give the parent thread a settled result.
    pub(super) async fn deliver(&self, task: &TaskId) -> Result<(), Error> {
        let _delivery = self.delivery.lock().await;
        let task = store::read(&mut *self.workspace.db.lock().await, task, None).await?;
        let Step::Deliver(mode) = decisions::next(&task.state) else {
            return Ok(());
        };
        let text = task.state.report(&task.id, &task.title, &task.environment);
        let wake = self
            .workspace
            .wake(task.parent, &task.id, WakeSource::Result, &text, mode)
            .await?;
        self.apply(&task.id, Command::Delivered(wake)).await
    }

    /// The child retains the outbox row until the parent acknowledges its
    /// durable wake. Retrying uses the same (task, continuation run) key.
    pub(super) async fn send_follow_up(
        &self,
        environment: &str,
        task: &TaskId,
        run: RunId,
    ) -> Result<(), Error> {
        let Some(mut row) = sql::row(
            &mut *self.workspace.db.lock().await,
            "SELECT thread, text FROM task_follow_ups WHERE environment = ?1 AND task = ?2 AND run = ?3",
            [environment.into(), task.into(), run.into()],
        ).await? else { return Ok(()) };
        let request = FollowUp {
            source: self.peers.local.clone(),
            task: task.clone(),
            thread: row.id(0)?,
            run,
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
        let (task, stopped) = {
            let mut db = self.workspace.db.lock().await;
            let task = match store::read(&mut db, &request.task, None).await {
                Ok(task) if task.environment == request.source => Some(task),
                Ok(_) | Err(Error::NoTask(_)) => None,
                Err(error) => return Err(error),
            };
            let stopped = match &task {
                Some(task) => match task.parent_run {
                    Some(run) => stop_exists(&mut db, task.parent, run).await?,
                    None => false,
                },
                None => false,
            };
            drop(db);
            (task, stopped)
        };
        let decision = decisions::follow_up(
            task.as_ref().map(|task| &task.state),
            request.thread,
            stopped,
        )?;
        let (task, mode) = match (decision, task) {
            (decisions::FollowUp::Later, _) => return Ok(false),
            (decisions::FollowUp::Discard, _) | (_, None) => return Ok(true),
            (decisions::FollowUp::Wake(mode), Some(task)) => (task, mode),
        };
        let text = format!(
            "Task {} ({}) on {} thread {} continued.\n\n{}",
            request.task, task.title, request.source, request.thread, request.text,
        );
        let source = WakeSource::Continuation(request.run);
        self.workspace
            .wake(task.parent, &request.task, source, &text, mode)
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
        let _delivery = self.delivery.lock().await;
        let mut db = self.workspace.db.lock().await;
        let family = snapshot(&mut db, thread, run).await?;
        let mut stopped = stop_exists(&mut db, thread, run).await?;
        if family.should_record_stop(cancel, stopped) {
            let mut tx = db.transaction().await?;
            sql::exec(
                &mut tx,
                "INSERT INTO stopped_task_runs (thread, run, handled) VALUES (?1, ?2, 0)",
                [thread.into(), run.into()],
            )
            .await?;
            suppress(&mut tx, thread, run).await?;
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

    pub(super) async fn drive_stop(&self, thread: ThreadId, run: RunId) {
        loop {
            match self.stop_family(thread, run).await {
                Ok(true) => return,
                Ok(false) => time::sleep(LOCAL_POLL).await,
                Err(error) => {
                    tracing::error!(%thread, %run, %error, "cancelling delegated run tree");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    async fn stop_family(&self, thread: ThreadId, run: RunId) -> Result<bool, Error> {
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
            suppress(&mut tx, thread, run).await?;
            tx.commit().await?;
            drop(db);
            family.active
        };
        for active_run in active {
            if let Err(error) = self.workspace.stop(thread, Some(active_run)).await
                && !error.rejected()
            {
                return Err(error);
            }
        }
        Ok(false)
    }
}
