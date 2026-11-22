//! Retry admission and progress asynchronously. The worker reloads a durable
//! phase after each peer request before applying the reply.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{task::JoinHandle, time};
use viz_core::tasks::{self as decisions, Command, Phase, Progress, Transition};
use viz_protocol::{
    RunId, ThreadId,
    peer::{ProgressRequest, ReceiveTask, RetractTask},
};

use super::{
    Job, LOCAL_POLL, POLL, Tasks,
    store::{finish_task, parse_status, task_phase},
};
use crate::{
    error::Error,
    wire::{Run, TaskStatus},
    workspace::sql,
};

impl Tasks {
    pub(super) async fn supervise(self: Arc<Self>) {
        let mut jobs: HashMap<Job, JoinHandle<()>> = HashMap::new();
        let mut poll = time::interval(POLL);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => break,
                _ = poll.tick() => {},
                () = self.scan.notified() => {},
            }
            jobs.retain(|_, worker| !worker.is_finished());
            match self.pending_jobs().await {
                Ok(pending) => {
                    for job in pending {
                        jobs.entry(job)
                            .or_insert_with_key(|job| self.start_job(job));
                    }
                }
                Err(error) => tracing::error!(%error, "scanning durable tasks"),
            }
        }
        for (_, worker) in jobs {
            let _ = worker.await;
        }
    }

    async fn pending_jobs(&self) -> Result<Vec<Job>, Error> {
        let mut db = self.workspace.db.lock().await;
        let mut jobs = sql::rows(
            &mut db,
            "SELECT id FROM agent_tasks WHERE delivery != 'delivered'",
            [],
        )
        .await?
        .map(|mut row| Ok(Job::Task(row.str(0)?)))
        .collect::<Result<Vec<_>, Error>>()?;
        for row in sql::rows(
            &mut db,
            "SELECT thread, run FROM stopped_task_runs WHERE handled = 0",
            [],
        )
        .await?
        {
            jobs.push(Job::Stop {
                thread: row.u64(0)?,
                run: row.u64(1)?,
            });
        }
        for row in sql::rows(
            &mut db,
            "SELECT environment, task, min(run) FROM task_follow_ups GROUP BY environment, task",
            [],
        )
        .await?
        {
            let mut row = row;
            jobs.push(Job::FollowUp {
                environment: row.str(0)?,
                task: row.str(1)?,
                run: row.u64(2)?,
            });
        }
        drop(db);
        Ok(jobs)
    }

    fn start_job(self: &Arc<Self>, job: &Job) -> JoinHandle<()> {
        let tasks = Arc::clone(self);
        match job {
            Job::Task(id) => {
                let id = id.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        () = tasks.drive_task(&id) => {},
                    }
                })
            }
            Job::FollowUp {
                environment,
                task,
                run,
            } => {
                let (environment, task, run) = (environment.clone(), task.clone(), *run);
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        result = tasks.send_follow_up(&environment, &task, run) => {
                            if let Err(error) = result {
                                tracing::debug!(%task, run, %error, "delivering task follow-up");
                            }
                        }
                    }
                })
            }
            Job::Stop { thread, run } => {
                let (thread, run) = (*thread, *run);
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        () = tasks.drive_stop(thread, run) => {},
                    }
                })
            }
        }
    }

    async fn drive_task(&self, id: &str) {
        loop {
            match self.advance(id).await {
                Ok(Advance::Done) => return,
                Ok(Advance::Immediate) => {}
                Ok(Advance::LocalPoll) => time::sleep(LOCAL_POLL).await,
                Ok(Advance::PeerPoll) => time::sleep(POLL).await,
                Err(error) => {
                    tracing::error!(task = id, %error, "advancing durable task");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    async fn advance(&self, id: &str) -> Result<Advance, Error> {
        let Some(mut row) = sql::row(&mut *self.workspace.db.lock().await,
            "SELECT environment, thread, run, status, phase FROM agent_tasks WHERE id = ?1 AND delivery != 'delivered'",
            [id.into()],
        ).await? else { return Ok(Advance::Done) };
        let phase = row.str(4)?.parse::<Phase>().map_err(Error::corrupt)?;
        if phase == Phase::Settled {
            self.deliver(id).await?;
            return Ok(Advance::Done);
        }
        let target = row.str(0)?;
        let cancel = phase == Phase::Cancelling;
        let local = target == self.peers.local;
        let was_reachable = self.peers.reachable(&target);
        let poll = if local {
            Advance::LocalPoll
        } else {
            Advance::PeerPoll
        };
        if matches!(phase, Phase::Pending | Phase::Offered | Phase::Retracting) {
            return self.advance_admission(id, &target, phase, poll).await;
        }
        let (Some(thread), Some(run)) = (row.opt_u64(1)?, row.opt_u64(2)?) else {
            return Err(Error::corrupt("accepted task has no run receipt"));
        };
        let status = parse_status(&row.str(3)?)?;
        let request = ProgressRequest {
            thread: ThreadId(thread),
            run: RunId(run),
            cancel,
        };
        let reply = if local {
            self.progress(request).await.map(|progress| progress.run)
        } else {
            self.peers
                .call::<ProgressRequest>(&target, &request, Duration::ZERO)
                .await
                .map(|progress| progress.run)
        };
        if was_reachable != self.peers.reachable(&target) {
            self.signal();
        }
        match reply {
            Ok(Some(progress)) if progress.thread.0 == thread && progress.id.0 == run => {
                self.update_from_run(id, status, &progress, poll).await
            }
            Ok(Some(_)) => {
                self.peers.mark(&target, false);
                self.signal();
                Ok(poll)
            }
            Ok(None) => {
                if cancel {
                    self.finish(id, TaskStatus::Cancelled, None, None).await?;
                } else {
                    self.finish(
                        id,
                        TaskStatus::Failed,
                        None,
                        Some("the assigned run no longer exists"),
                    )
                    .await?;
                }
                Ok(Advance::Immediate)
            }
            Err(error) if local => Err(error),
            Err(_) => Ok(poll),
        }
    }

    async fn advance_admission(
        &self,
        id: &str,
        target: &str,
        phase: Phase,
        poll: Advance,
    ) -> Result<Advance, Error> {
        let was_reachable = self.peers.reachable(target);
        let retracting = phase == Phase::Retracting;
        let result = if retracting {
            let request = RetractTask {
                origin_environment: self.peers.local.clone(),
                task: id.to_owned(),
            };
            if target == self.peers.local {
                self.workspace.retract(request).await
            } else {
                self.peers
                    .call::<RetractTask>(target, &request, Duration::ZERO)
                    .await
            }
        } else {
            let payload = {
                let mut db = self.workspace.db.lock().await;
                let Some(current) = task_phase(&mut db, id).await? else {
                    return Ok(Advance::Done);
                };
                if let Transition::Move(next) = decisions::decide(current, Command::Offer) {
                    sql::exec(
                        &mut db,
                        "UPDATE agent_tasks SET phase = ?2 WHERE id = ?1",
                        [id.into(), next.as_str().into()],
                    )
                    .await?;
                } else if current != Phase::Offered {
                    return Ok(Advance::Immediate);
                }
                let payload = sql::row(
                    &mut db,
                    "SELECT payload FROM agent_tasks WHERE id = ?1 AND phase = 'offered'",
                    [id.into()],
                )
                .await?;
                drop(db);
                let Some(mut row) = payload else {
                    return Ok(Advance::Immediate);
                };
                row.str(0)?
            };
            let request: ReceiveTask = serde_json::from_str(&payload)?;
            if target == self.peers.local {
                self.workspace.receive(request).await.map(Some)
            } else {
                self.peers
                    .call::<ReceiveTask>(target, &request, Duration::ZERO)
                    .await
                    .map(Some)
            }
        };
        if was_reachable != self.peers.reachable(target) {
            self.signal();
        }
        match result {
            Ok(Some(receipt)) => {
                let mut db = self.workspace.db.lock().await;
                if let Some(current) = task_phase(&mut db, id).await?
                    && let Transition::Move(next) = decisions::decide(current, Command::Receipt)
                {
                    sql::exec(&mut db, "UPDATE agent_tasks SET thread = ?2, run = ?3, payload = NULL, phase = ?4 WHERE id = ?1",
                        [id.into(), receipt.thread.get().into(), receipt.run.get().into(), next.as_str().into()]).await?;
                }
                drop(db);
                self.signal();
                Ok(Advance::Immediate)
            }
            Ok(None) if retracting => {
                self.admission_failed(id, Command::Retracted, None).await?;
                Ok(Advance::Immediate)
            }
            Err(error) if !retracting && error.rejected() => {
                self.admission_failed(id, Command::Rejected, Some(&error.to_string()))
                    .await?;
                Ok(Advance::Immediate)
            }
            _ => Ok(poll),
        }
    }

    async fn admission_failed(
        &self,
        id: &str,
        command: Command,
        error: Option<&str>,
    ) -> Result<(), Error> {
        let mut db = self.workspace.db.lock().await;
        if let Some(current) = task_phase(&mut db, id).await?
            && let Transition::Settle(status) = decisions::decide(current, command)
        {
            let error = error.filter(|_| status == TaskStatus::Failed);
            finish_task(&mut db, id, status, None, error).await?;
        }
        drop(db);
        self.signal();
        Ok(())
    }

    async fn update_from_run(
        &self,
        id: &str,
        previous: TaskStatus,
        run: &Run,
        poll: Advance,
    ) -> Result<Advance, Error> {
        match decisions::record_progress(previous, run) {
            Progress::Wait => Ok(poll),
            Progress::Started => {
                sql::exec(
                    &mut *self.workspace.db.lock().await,
                    "UPDATE agent_tasks SET status = 'running' WHERE id = ?1",
                    [id.into()],
                )
                .await?;
                self.signal();
                Ok(poll)
            }
            Progress::Settled {
                status,
                answer,
                error,
            } => {
                self.finish(id, status, answer, error).await?;
                Ok(Advance::Immediate)
            }
        }
    }

    async fn finish(
        &self,
        id: &str,
        status: TaskStatus,
        result: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), Error> {
        finish_task(
            &mut *self.workspace.db.lock().await,
            id,
            status,
            result,
            error,
        )
        .await?;
        self.signal();
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Advance {
    Done,
    Immediate,
    LocalPoll,
    PeerPoll,
}
