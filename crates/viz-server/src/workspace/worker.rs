//! A worker owns one thread's ACP session. Its mailbox serializes configuration
//! and checkout changes with prompting. Run controls use a coalescing watch
//! channel so stop can interrupt an effect without waiting behind
//! configuration.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{Notify, mpsc, oneshot, watch},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use viz_core::runs::EndStatus;
use viz_protocol::{OptionValue, RunId, ThreadId};

use super::{
    Workspace, runs, runtime::settle_outcome, sessions::SessionSlot, sql::exec, store,
    worktree::Placement,
};
use crate::wire::RpcError;

/// Restorable idle processes are released after ten minutes.
const IDLE_SESSION: Duration = Duration::from_mins(10);

#[derive(Default)]
pub(super) struct Workers(Mutex<HashMap<ThreadId, Arc<Driver>>>);

impl Workers {
    pub(super) fn get(&self, thread: ThreadId) -> Option<Arc<Driver>> {
        self.0
            .lock()
            .expect("worker registry poisoned")
            .get(&thread)
            .cloned()
    }

    pub(super) fn remove(&self, thread: ThreadId) -> Option<Arc<Driver>> {
        self.0
            .lock()
            .expect("worker registry poisoned")
            .remove(&thread)
    }
}

pub(super) struct Driver {
    pub(super) lifetime: CancellationToken,
    pub(super) queued: Notify,
    prepare: AtomicBool,
    inbox: mpsc::UnboundedSender<Request>,
    active: watch::Sender<Option<Active>>,
    done: watch::Receiver<()>,
}

struct Worker {
    inbox: mpsc::UnboundedReceiver<Request>,
    slot: SessionSlot,
    /// Drops only after the slot, acknowledging that the process was released.
    _done: watch::Sender<()>,
}

enum Request {
    Configure {
        option: String,
        value: OptionValue,
        reply: oneshot::Sender<Result<(), RpcError>>,
    },
    Rebind {
        before: Box<store::Binding>,
        after: Placement,
        reply: oneshot::Sender<Result<(), RpcError>>,
    },
}

#[derive(Clone, Copy)]
struct Active {
    run: RunId,
    intent: Intent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Intent {
    Continue,
    Steer,
    Stop,
}

pub(super) struct Turn {
    pub(super) run: RunId,
    pub(super) cancel: CancellationToken,
    pub(super) steer: CancellationToken,
}

impl Driver {
    fn new(lifetime: CancellationToken) -> (Self, Worker) {
        let (send, inbox) = mpsc::unbounded_channel();
        let (closed, done) = watch::channel(());
        (
            Self {
                lifetime,
                queued: Notify::new(),
                prepare: AtomicBool::new(false),
                inbox: send,
                active: watch::Sender::new(None),
                done,
            },
            Worker {
                inbox,
                slot: SessionSlot::default(),
                _done: closed,
            },
        )
    }

    fn wake(&self, prepare: bool) {
        if prepare {
            self.prepare.store(true, Ordering::Relaxed);
        }
        self.queued.notify_one();
    }

    pub(super) fn is_current(&self, run: RunId) -> bool {
        self.active.borrow().is_some_and(|active| active.run == run)
    }

    pub(super) fn signal(&self, run: RunId, intent: Intent) {
        self.active.send_if_modified(|active| {
            let Some(active) = active.as_mut().filter(|active| active.run == run) else {
                return false;
            };
            if active.intent == Intent::Stop || active.intent == intent {
                return false;
            }
            active.intent = intent;
            true
        });
    }

    pub(super) fn stopping(&self, run: RunId) -> bool {
        self.lifetime.is_cancelled()
            || self
                .active
                .borrow()
                .is_some_and(|active| active.run == run && active.intent == Intent::Stop)
    }

    /// Call under the write lease immediately after committing the claim.
    pub(super) fn begin(&self, run: RunId) -> Turn {
        self.active.send_replace(Some(Active {
            run,
            intent: Intent::Continue,
        }));
        Turn {
            run,
            cancel: self.lifetime.child_token(),
            steer: CancellationToken::new(),
        }
    }

    pub(super) fn end(&self) {
        self.active.send_replace(None);
    }

    /// Translate the worker intent into the ACP adapter's cancellation handles.
    pub(super) async fn interpret<T>(&self, turn: &Turn, effect: impl Future<Output = T>) -> T {
        let mut signals = self.active.subscribe();
        tokio::pin!(effect);
        loop {
            let active = *signals.borrow_and_update();
            if let Some(active) = active {
                match active.intent {
                    Intent::Continue => {}
                    Intent::Steer => turn.steer.cancel(),
                    Intent::Stop => turn.cancel.cancel(),
                }
            }
            tokio::select! {
                biased;
                _ = signals.changed() => {},
                result = &mut effect => return result,
            }
        }
    }

    async fn request(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<(), RpcError>>) -> Request,
    ) -> Result<(), RpcError> {
        let (reply, result) = oneshot::channel();
        self.inbox.send(make(reply)).map_err(|_| worker_closed())?;
        result.await.map_err(|_| worker_closed())?
    }

    pub(super) async fn rebind(
        &self,
        before: store::Binding,
        after: Placement,
    ) -> Result<(), RpcError> {
        self.request(move |reply| Request::Rebind {
            before: Box::new(before),
            after,
            reply,
        })
        .await
    }

    pub(super) async fn close(&self) {
        self.lifetime.cancel();
        let _ = self.done.clone().changed().await;
    }
}

fn worker_closed() -> RpcError {
    RpcError::unavailable("thread worker stopped")
}

impl Workspace {
    /// The caller holds the workspace write lease while resolving a thread so
    /// deletion cannot race insertion of its driver.
    pub(super) fn driver(self: &Arc<Self>, thread: ThreadId) -> Result<Arc<Driver>, RpcError> {
        let mut workers = self.workers.0.lock().expect("worker registry poisoned");
        if self.closing.is_cancelled() {
            return Err(worker_closed());
        }
        Ok(Arc::clone(workers.entry(thread).or_insert_with(|| {
            let (driver, worker) = Driver::new(self.closing.child_token());
            let driver = Arc::new(driver);
            let workspace = Arc::clone(self);
            let handle = Arc::clone(&driver);
            self.jobs
                .spawn(async move { workspace.worker(thread, &handle, worker).await });
            driver
        })))
    }

    pub(super) fn kick(self: &Arc<Self>, thread: u64, prepare: bool) {
        if let Ok(driver) = self.driver(ThreadId(thread)) {
            driver.wake(prepare);
        }
    }

    pub(crate) async fn shutdown(&self) {
        {
            let _workers = self.workers.0.lock().expect("worker registry poisoned");
            self.closing.cancel();
            self.jobs.close();
        }
        self.jobs.wait().await;
        self.workers
            .0
            .lock()
            .expect("worker registry poisoned")
            .clear();
    }

    pub(super) async fn configure(
        self: &Arc<Self>,
        thread: u64,
        option: String,
        value: OptionValue,
    ) -> Result<(), RpcError> {
        let driver = {
            let mut db = self.db.lock().await;
            store::require(&mut db, thread).await?;
            let driver = self.driver(ThreadId(thread))?;
            drop(db);
            driver
        };
        driver
            .request(move |reply| Request::Configure {
                option,
                value,
                reply,
            })
            .await
    }

    async fn worker(&self, thread: ThreadId, driver: &Driver, mut worker: Worker) {
        let mut ready = false;
        let mut idle_at = Instant::now() + IDLE_SESSION;
        loop {
            let restorable = worker.slot.is_restorable();
            tokio::select! {
                biased;
                () = driver.lifetime.cancelled() => break,
                request = worker.inbox.recv() => {
                    if let Some(request) = request {
                        self.handle_request(thread, driver, &mut worker.slot, request).await;
                        idle_at = Instant::now() + IDLE_SESSION;
                    } else {
                        break;
                    }
                }
                changed = worker.slot.changed() => {
                    if changed {
                        if let Err(error) = self.refresh_session(thread.0, &worker.slot).await {
                            tracing::error!(%thread, %error, "saving ACP session update");
                        }
                        ready = true;
                    }
                }
                () = driver.queued.notified() => ready = true,
                () = std::future::ready(()), if ready => {
                    let prepare = driver.prepare.swap(false, Ordering::Relaxed);
                    ready = match self.step(thread, driver, &mut worker.slot, prepare).await {
                        Ok(more) => more,
                        Err(error) => {
                            driver.end();
                            tracing::error!(%thread, %error, "ACP scheduler failed");
                            false
                        }
                    };
                    idle_at = Instant::now() + IDLE_SESSION;
                }
                () = sleep_until(idle_at), if restorable => worker.slot.clear(),
            }
        }
    }

    async fn handle_request(
        &self,
        thread: ThreadId,
        driver: &Driver,
        slot: &mut SessionSlot,
        request: Request,
    ) {
        match request {
            Request::Configure {
                option,
                value,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self
                        .configure_session(thread.0, slot, &driver.lifetime, &option, value)
                        .await;
                    let _ = reply.send(result);
                }
            }
            Request::Rebind {
                before,
                after,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self.apply_checkout(thread.0, &before, &after).await;
                    if result.is_ok() {
                        slot.clear();
                        driver.wake(true);
                    }
                    let _ = reply.send(result);
                }
            }
        }
    }

    async fn step(
        &self,
        thread: ThreadId,
        driver: &Driver,
        slot: &mut SessionSlot,
        prepare: bool,
    ) -> Result<bool, RpcError> {
        if let Some(session) = slot.current.as_mut() {
            self.continue_idle(thread.0, driver, session).await?;
        }
        let claim = {
            let mut db = self.db.lock().await;
            if driver.lifetime.is_cancelled() {
                return Ok(false);
            }
            let mut tx = db.transaction().await?;
            let claim = runs::claim(&mut tx, thread.0).await?;
            tx.commit().await?;
            let claim = claim.map(|claim| {
                let turn = driver.begin(RunId(claim.id));
                (claim, turn)
            });
            drop(db);
            claim
        };
        if let Some((claim, turn)) = claim {
            self.changed(Some(thread.0));
            let result = driver
                .interpret(&turn, self.prompt(thread.0, slot, claim, &turn))
                .await;
            let settlement = settle_outcome(result, &self.closing, &turn.cancel, &turn.steer);
            if settlement.status == EndStatus::Interrupted {
                slot.clear();
            }
            if let Err(error) = self.finish(thread.0, driver, &turn, settlement).await {
                self.abandon(thread.0, turn.run.0, &error).await;
                return Err(error);
            }
            driver.end();
            return Ok(true);
        }
        if !prepare {
            return Ok(false);
        }
        let result = self
            .session(thread.0, slot, &driver.lifetime)
            .await
            .map(drop);
        if let Err(error) = result
            && !driver.lifetime.is_cancelled()
        {
            tracing::warn!(%thread, %error, "ACP thread setup failed");
            exec(
                &mut *self.db.lock().await,
                "UPDATE threads SET setup_failed = 1 WHERE id = ?1",
                [thread.0.into()],
            )
            .await?;
            self.relist(thread.0);
        }
        Ok(true)
    }
}
