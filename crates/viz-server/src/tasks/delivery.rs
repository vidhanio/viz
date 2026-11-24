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
use crate::{
    error::Error,
    workspace::{NewWake, Woken, sql},
};

impl Tasks {
    /// Give the parent thread a settled result.
    pub(super) async fn deliver(&self, task: &TaskId) -> Result<(), Error> {
        let _delivery = self.delivery.lock().await;
        let task_id = task.clone();
        let task = self
            .workspace
            .db
            .read(move |conn| store::read(conn, &task_id, None))
            .await?;
        let Step::Deliver(mode) = decisions::next(&task.state) else {
            return Ok(());
        };
        let text = task.state.report(&task.id, &task.title, &task.environment);
        let wake = NewWake {
            thread: task.parent,
            task: task.id.clone(),
            source: WakeSource::Result,
            text,
            mode,
        };
        let state = task.state;
        let id = task.id;
        self.workspace
            .wake(wake, move |tx, woken| {
                let run = match woken {
                    Woken::Run(run) => Some(*run),
                    Woken::Entry => None,
                    Woken::Deleted => return store::remove(tx, &id),
                };
                decisions::decide(&state, Command::Delivered(run))
                    .map_or_else(|| Ok(()), |next| store::save(tx, &id, &next))
            })
            .await?;
        self.signal();
        Ok(())
    }

    /// The child retains the outbox row until the parent acknowledges its
    /// durable wake. Retrying uses the same (task, continuation run) key.
    pub(super) async fn send_follow_up(
        &self,
        environment: &str,
        task: &TaskId,
        run: RunId,
    ) -> Result<(), Error> {
        let env_owned = environment.to_owned();
        let task_owned = task.clone();
        let Some((thread, text)) = self
            .workspace
            .db
            .read(move |conn| {
                sql::row(
                    conn,
                    "SELECT thread, text FROM task_follow_ups WHERE environment = ?1 AND task = ?2 AND run = ?3",
                    [env_owned.as_str().into(), (&task_owned).into(), run.into()],
                )?
                .map(|mut row| Ok::<_, Error>((row.id(0)?, row.str(1)?)))
                .transpose()
            })
            .await?
        else {
            return Ok(());
        };
        let request = FollowUp {
            source: self.peers.local.clone(),
            task: task.clone(),
            thread,
            run,
            text,
        };
        let acknowledged = if environment == self.peers.local {
            self.follow_up(request).await?
        } else {
            self.peers
                .call::<FollowUp>(environment, &request, Duration::ZERO)
                .await?
        };
        if acknowledged {
            let env_owned = environment.to_owned();
            let task_owned = task.clone();
            self.workspace
                .db
                .write(move |conn| {
                    sql::exec(
                        conn,
                        "DELETE FROM task_follow_ups WHERE environment = ?1 AND task = ?2 AND run = ?3",
                        [env_owned.as_str().into(), (&task_owned).into(), run.into()],
                    )?;
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }

    /// Deliver the original result first. A stopped parent run can receive
    /// transcript entries, but cannot be woken by a follow-up.
    pub(crate) async fn follow_up(&self, request: FollowUp) -> Result<bool, Error> {
        let _delivery = self.delivery.lock().await;
        let task_id = request.task.clone();
        let source = request.source.clone();
        let (task, stopped) = self
            .workspace
            .db
            .read_snapshot(move |tx| {
                let task = match store::read(tx, &task_id, None) {
                    Ok(task) if task.environment == source => Some(task),
                    Ok(_) | Err(Error::NoTask(_)) => None,
                    Err(error) => return Err(error),
                };
                let stopped = match &task {
                    Some(task) => match task.parent_run {
                        Some(run) => stop_exists(tx, task.parent, run)?,
                        None => false,
                    },
                    None => false,
                };
                Ok((task, stopped))
            })
            .await?;
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
        let wake = NewWake {
            thread: task.parent,
            task: request.task,
            source: WakeSource::Continuation(request.run),
            text,
            mode,
        };
        self.workspace.wake(wake, |_, _| Ok(())).await?;
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
        let (run_progress, recorded_stop) = self
            .workspace
            .db
            .write(move |conn| {
                let family = snapshot(conn, thread, run)?;
                let mut stopped = stop_exists(conn, thread, run)?;
                let recorded_stop = if family.should_record_stop(cancel, stopped) {
                    let tx = sql::immediate(conn)?;
                    sql::exec(
                        &tx,
                        "INSERT INTO stopped_task_runs (thread, run, handled) VALUES (?1, ?2, 0)",
                        [thread.into(), run.into()],
                    )?;
                    suppress(&tx, thread, run)?;
                    tx.commit()?;
                    stopped = true;
                    true
                } else {
                    false
                };
                Ok((family.progress(thread, run, stopped), recorded_stop))
            })
            .await?;
        if recorded_stop {
            self.scan.notify_one();
            self.signal();
        }
        Ok(ProgressResult { run: run_progress })
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
            let outcome = self
                .workspace
                .db
                .write(move |conn| {
                    let family = snapshot(conn, thread, run)?;
                    if !family.pending() {
                        sql::exec(
                            conn,
                            "UPDATE stopped_task_runs SET handled = 1 WHERE thread = ?1 AND run = ?2",
                            [thread.into(), run.into()],
                        )?;
                        return Ok(None);
                    }
                    let tx = sql::immediate(conn)?;
                    suppress(&tx, thread, run)?;
                    tx.commit()?;
                    Ok(Some(family.active))
                })
                .await?;
            match outcome {
                Some(active) => active,
                None => return Ok(true),
            }
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
