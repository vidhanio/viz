//! Drive persisted tasks to delivery. Each job repeats until its durable
//! record is done, across restarts and partitions.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{sync::broadcast, task::JoinHandle, time};
use viz_core::{
    runs::is_open,
    tasks::{self as decisions, Command, Step},
};
use viz_protocol::{
    TaskId, ThreadId,
    peer::{ProgressRequest, ReceiveTask, Received, RetractTask},
};

use super::{Job, LOCAL_POLL, POLL, Tasks, store};
use crate::{
    error::Error,
    workspace::{Change, sql},
};

const MAX_CONCURRENT_JOBS: usize = 64;

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
            if jobs.len() >= MAX_CONCURRENT_JOBS {
                continue;
            }
            match self.pending_jobs().await {
                Ok(pending) => {
                    for job in pending {
                        if jobs.len() >= MAX_CONCURRENT_JOBS {
                            break;
                        }
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
        .map(|mut row| Ok(Job::Task(TaskId(row.str(0)?))))
        .collect::<Result<Vec<_>, Error>>()?;
        for row in sql::rows(
            &mut db,
            "SELECT thread, run FROM stopped_task_runs WHERE handled = 0",
            [],
        )
        .await?
        {
            jobs.push(Job::Stop {
                thread: row.id(0)?,
                run: row.id(1)?,
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
                task: TaskId(row.str(1)?),
                run: row.id(2)?,
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
                    tasks.scan.notify_one();
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
                                tracing::debug!(%task, %run, %error, "delivering task follow-up");
                            }
                        }
                    }
                    tasks.scan.notify_one();
                })
            }
            Job::Stop { thread, run } => {
                let (thread, run) = (*thread, *run);
                tokio::spawn(async move {
                    tokio::select! {
                        () = tasks.shutdown.cancelled() => {},
                        () = tasks.drive_stop(thread, run) => {},
                    }
                    tasks.scan.notify_one();
                })
            }
        }
    }

    async fn drive_task(&self, id: &TaskId) {
        loop {
            match self.advance(id).await {
                Ok(Advance::Done) => return,
                Ok(Advance::Immediate) => {}
                Ok(Advance::LocalPoll) => time::sleep(LOCAL_POLL).await,
                Ok(Advance::Settling(settling)) => settling.wait().await,
                Ok(Advance::PeerPoll) => time::sleep(POLL).await,
                Err(error) => {
                    tracing::error!(task = %id, %error, "advancing durable task");
                    time::sleep(POLL).await;
                }
            }
        }
    }

    /// Take the persisted task's next step, and record what came of it.
    async fn advance(&self, id: &TaskId) -> Result<Advance, Error> {
        let task = match store::read(&mut *self.workspace.db.lock().await, id, None).await {
            Ok(task) => task,
            Err(Error::NoTask(_)) => return Ok(Advance::Done),
            Err(error) => return Err(error),
        };
        let target = task.environment.as_str();
        let was_reachable = self.peers.reachable(target);
        let advance = match decisions::next(&task.state) {
            Step::Offer => {
                self.apply(id, Command::Offer).await?;
                Ok(Advance::Immediate)
            }
            Step::Send => self.offer(id, target).await,
            Step::Retract => self.retract(id, target).await,
            Step::Poll { receipt, cancel } => self.poll(id, target, receipt, cancel).await,
            Step::Deliver(_) if self.held(id) => Ok(Advance::LocalPoll),
            Step::Deliver(_) => {
                self.deliver(id).await?;
                Ok(Advance::Done)
            }
            Step::Done => Ok(Advance::Done),
        };
        if was_reachable != self.peers.reachable(target) {
            self.signal();
        }
        advance
    }

    /// An answer that never arrives decides nothing: the offer is repeated
    /// until the receiver admits or refuses it, or it is retracted.
    async fn offer(&self, id: &TaskId, target: &str) -> Result<Advance, Error> {
        let payload = store::payload(&mut *self.workspace.db.lock().await, id).await?;
        let Some(payload) = payload else {
            return Ok(Advance::Immediate);
        };
        let request: ReceiveTask = serde_json::from_str(&payload).map_err(Error::corrupt)?;
        let result = if self.is_local(target) {
            self.workspace.receive(request).await
        } else {
            let mut upload_err = None;
            for attachment in &request.attachments {
                match self
                    .workspace
                    .read_blob(&attachment.blob, attachment.size)
                    .await
                {
                    Ok(bytes) => {
                        if let Err(error) =
                            self.peers.push_blob(target, &attachment.blob, bytes).await
                        {
                            upload_err = Some(error);
                            break;
                        }
                    }
                    Err(error) => {
                        upload_err = Some(error);
                        break;
                    }
                }
            }
            match upload_err {
                Some(error) => Err(error),
                None => {
                    self.peers
                        .call::<ReceiveTask>(target, &request, Duration::ZERO)
                        .await
                }
            }
        };
        match result {
            Ok(receipt) => self.apply(id, Command::Admitted(receipt)).await?,
            Err(error) if error.rejected() => {
                self.apply(id, Command::Refused(&error.to_string())).await?;
            }
            Err(_) => return Ok(self.pause(target)),
        }
        Ok(Advance::Immediate)
    }

    async fn retract(&self, id: &TaskId, target: &str) -> Result<Advance, Error> {
        let request = RetractTask {
            origin_environment: self.peers.local.clone(),
            task: id.clone(),
        };
        let result = if self.is_local(target) {
            self.workspace.retract(request).await
        } else {
            self.peers
                .call::<RetractTask>(target, &request, Duration::ZERO)
                .await
        };
        match result {
            Ok(Some(receipt)) => self.apply(id, Command::Admitted(receipt)).await?,
            Ok(None) => self.apply(id, Command::Retracted).await?,
            Err(_) => return Ok(self.pause(target)),
        }
        Ok(Advance::Immediate)
    }

    async fn poll(
        &self,
        id: &TaskId,
        target: &str,
        receipt: Received,
        cancel: bool,
    ) -> Result<Advance, Error> {
        let request = ProgressRequest {
            thread: receipt.thread,
            run: receipt.run,
            cancel,
        };
        let settling = self.is_local(target).then(|| Settling {
            thread: receipt.thread,
            runs: self.workspace.subscribe(),
            tasks: self.changed.subscribe(),
        });
        let reply = if self.is_local(target) {
            self.progress(request).await?
        } else {
            match self
                .peers
                .call::<ProgressRequest>(target, &request, Duration::ZERO)
                .await
            {
                Ok(reply) => reply,
                Err(_) => return Ok(self.pause(target)),
            }
        };
        match reply.run {
            None => self.apply(id, Command::Vanished).await?,
            Some(run) if (run.thread, run.id) == (receipt.thread, receipt.run) => {
                self.apply(id, Command::Progressed(&run)).await?;
                if is_open(run.status) {
                    return Ok(settling.map_or_else(|| self.pause(target), Advance::Settling));
                }
            }
            Some(_) => {
                self.peers.mark(target, false);
                return Ok(self.pause(target));
            }
        }
        Ok(Advance::Immediate)
    }

    fn is_local(&self, target: &str) -> bool {
        target == self.peers.local
    }

    fn pause(&self, target: &str) -> Advance {
        if self.is_local(target) {
            Advance::LocalPoll
        } else {
            Advance::PeerPoll
        }
    }

    /// Decide and store what `command` does to the task as it is now, which
    /// may differ from when the step was chosen.
    pub(super) async fn apply(&self, id: &TaskId, command: Command<'_>) -> Result<(), Error> {
        let changed = {
            let mut db = self.workspace.db.lock().await;
            let mut tx = db.transaction().await?;
            let task = store::read(&mut tx, id, None).await?;
            let next = decisions::decide(&task.state, command);
            if let Some(next) = &next {
                store::save(&mut tx, id, next).await?;
            }
            tx.commit().await?;
            drop(db);
            next.is_some()
        };
        if changed {
            self.signal();
        }
        Ok(())
    }
}

enum Advance {
    Done,
    Immediate,
    LocalPoll,
    PeerPoll,
    /// A local run is still open: look again once it settles.
    Settling(Settling),
}

/// Waits for a local delegated run to settle, for any task to change (such
/// as a cancellation), or for [`POLL`] to pass, whichever comes first. The
/// receivers are subscribed before the run's progress is read, so a settlement
/// in between is not missed.
struct Settling {
    thread: ThreadId,
    runs: broadcast::Receiver<Change>,
    tasks: broadcast::Receiver<()>,
}

impl Settling {
    async fn wait(mut self) {
        let thread = self.thread;
        let settled = async {
            loop {
                match self.runs.recv().await {
                    Ok(Change::Settled(settled)) if settled == thread => return,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => {
                        std::future::pending::<()>().await;
                    }
                }
            }
        };
        let changed = async {
            match self.tasks.recv().await {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            }
        };
        tokio::select! {
            () = settled => {},
            () = changed => {},
            () = time::sleep(POLL) => {},
        }
    }
}
