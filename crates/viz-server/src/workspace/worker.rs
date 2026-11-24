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
use viz_core::lifecycle::on_run_end;
use viz_protocol::{OptionValue, RunId, RunStatus, ThreadId};

use super::{
    Workspace, runs,
    runtime::turn_end,
    sessions::{self, SessionSlot},
    sql::{self, exec},
    store,
    worktree::Placement,
};
use crate::error::Error;

/// Restorable idle processes are released after ten minutes.
const IDLE_SESSION: Duration = Duration::from_mins(10);
const MAX_RETRY: Duration = Duration::from_mins(1);

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

    /// Asks every worker to look at its queue again.
    pub(super) fn wake_all(&self) {
        for driver in self.0.lock().expect("worker registry poisoned").values() {
            driver.wake(false);
        }
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

/// When to repeat a step that failed: after one second, then twice as long
/// each time.
#[derive(Default)]
struct Retry {
    at: Option<Instant>,
    delay: Duration,
}

impl Retry {
    fn schedule(&mut self) {
        self.delay = (self.delay * 2).clamp(Duration::from_secs(1), MAX_RETRY);
        self.at = Some(Instant::now() + self.delay);
    }

    async fn due(&mut self) {
        match self.at {
            Some(at) => sleep_until(at).await,
            None => std::future::pending().await,
        }
        self.at = None;
    }
}

enum Request {
    Configure {
        option: String,
        value: OptionValue,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Rebind {
        before: Box<store::Binding>,
        after: Placement,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Title(String),
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

    /// The run under way, if any.
    pub(super) fn active_run(&self) -> Option<RunId> {
        self.active.borrow().map(|active| active.run)
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
        make: impl FnOnce(oneshot::Sender<Result<(), Error>>) -> Request,
    ) -> Result<(), Error> {
        let (reply, result) = oneshot::channel();
        self.inbox.send(make(reply)).map_err(|_| worker_closed())?;
        result.await.map_err(|_| worker_closed())?
    }

    pub(super) async fn rebind(
        &self,
        before: store::Binding,
        after: Placement,
    ) -> Result<(), Error> {
        self.request(move |reply| Request::Rebind {
            before: Box::new(before),
            after,
            reply,
        })
        .await
    }

    /// Names the open session after the user's title once the worker is free.
    pub(super) fn title(&self, title: String) {
        let _ = self.inbox.send(Request::Title(title));
    }

    pub(super) async fn close(&self) {
        self.lifetime.cancel();
        let _ = self.done.clone().changed().await;
    }
}

fn worker_closed() -> Error {
    Error::unavailable("thread worker stopped")
}

impl Workspace {
    /// The caller holds the workspace write lease while resolving a thread so
    /// deletion cannot race insertion of its driver.
    pub(super) fn driver(self: &Arc<Self>, thread: ThreadId) -> Result<Arc<Driver>, Error> {
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

    pub(super) fn kick(self: &Arc<Self>, thread: ThreadId, prepare: bool) {
        if let Ok(driver) = self.driver(thread) {
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

    /// Applies an option at once when the worker is free. A turn under way
    /// holds the session, so a request made during one, or overtaken by one,
    /// is saved as the thread's choice and applied when the turn ends; the
    /// caller, often answering or changing access for that very turn, does not
    /// wait for it.
    pub(super) async fn configure(
        self: &Arc<Self>,
        thread: ThreadId,
        option: String,
        value: OptionValue,
    ) -> Result<(), Error> {
        let ws = Arc::clone(self);
        let (driver, options) = self
            .db
            .write(move |conn| {
                let options = store::binding(conn, thread)?.session.options;
                let driver = ws.driver(thread)?;
                Ok((driver, options))
            })
            .await?;
        if !options.is_empty() && !viz_core::options::offers(&options, &option, &value) {
            return Err(Error::invalid(format!("invalid value for option {option}")));
        }
        let (reply, result) = oneshot::channel();
        driver
            .inbox
            .send(Request::Configure {
                option: option.clone(),
                value: value.clone(),
                reply,
            })
            .map_err(|_| worker_closed())?;
        let mut active = driver.active.subscribe();
        tokio::select! {
            biased;
            result = result => result.map_err(|_| worker_closed())?,
            true = async { active.wait_for(Option::is_some).await.is_ok() } => {
                let relist = self
                    .db
                    .write(move |conn| sessions::defer_option(conn, thread, &option, value))
                    .await?;
                if relist {
                    self.relist(thread);
                } else {
                    self.changed(Some(thread));
                }
                Ok(())
            }
        }
    }

    async fn worker(&self, thread: ThreadId, driver: &Arc<Driver>, mut worker: Worker) {
        let mut ready = false;
        let mut retry = Retry::default();
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
                        if let Err(error) = self.refresh_session(thread, &worker.slot).await {
                            tracing::error!(%thread, %error, "saving ACP session update");
                        }
                        ready = true;
                    }
                }
                () = driver.queued.notified() => ready = true,
                () = std::future::ready(()), if ready => {
                    let prepare = driver.prepare.swap(false, Ordering::Relaxed);
                    ready = match self.step(thread, driver, &mut worker.slot, prepare).await {
                        Ok(more) => {
                            retry = Retry::default();
                            more
                        }
                        Err(error) => {
                            driver.end();
                            tracing::error!(%thread, %error, "ACP scheduler failed");
                            if prepare {
                                driver.prepare.store(true, Ordering::Relaxed);
                            }
                            retry.schedule();
                            false
                        }
                    };
                    idle_at = Instant::now() + IDLE_SESSION;
                }
                () = retry.due() => ready = true,
                () = sleep_until(idle_at), if restorable => worker.slot.close().await,
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
                let result = self
                    .configure_session(thread, slot, &driver.lifetime, &option, value)
                    .await;
                if let Err(error) = &result
                    && reply.is_closed()
                {
                    tracing::warn!(%thread, %option, %error, "applying a deferred option");
                }
                let _ = reply.send(result);
            }
            Request::Rebind {
                before,
                after,
                reply,
            } => {
                if !reply.is_closed() {
                    let result = self.apply_checkout(thread, &before, &after).await;
                    if result.is_ok() {
                        slot.clear();
                        driver.wake(true);
                    }
                    let _ = reply.send(result);
                }
            }
            Request::Title(title) => self.title_session(thread, slot, &title).await,
        }
    }

    async fn step(
        &self,
        thread: ThreadId,
        driver: &Arc<Driver>,
        slot: &mut SessionSlot,
        prepare: bool,
    ) -> Result<bool, Error> {
        if let Some(session) = slot.current.as_mut() {
            self.continue_idle(thread, driver, session).await?;
        }
        let root = self.turn_root(thread).await?;
        let driver_for_write = Arc::clone(driver);
        let reservations = Arc::clone(&self.reservations);
        let (claim, recovered) = self
            .db
            .write(move |conn| {
                if driver_for_write.lifetime.is_cancelled() {
                    return Ok((None, false));
                }
                let occupancy = reservations.occupy(root.as_deref());
                let tx = sql::immediate(conn)?;
                let found = match &occupancy {
                    Some(_) => runs::claim(&tx, thread)?,
                    None => runs::Found::default(),
                };
                tx.commit()?;
                let claim = found.claimed.zip(occupancy).map(|(claim, occupancy)| {
                    let turn = driver_for_write.begin(claim.id);
                    (claim, turn, occupancy)
                });
                Ok((claim, found.recovered))
            })
            .await?;
        if recovered {
            self.settled(thread);
        }
        if let Some((claim, turn, _occupancy)) = claim {
            self.changed(Some(thread));
            self.checkpoint(thread, turn.run, viz_protocol::CheckpointEdge::Before)
                .await;
            let result = driver
                .interpret(&turn, Box::pin(self.prompt(thread, slot, claim, &turn)))
                .await;
            if let Err(error) = &result {
                self.harness_failed(thread, error).await;
            }
            if let Err(error) = self.finish(thread, driver, &turn, turn_end(result)).await {
                self.abandon(thread, turn.run, &error).await;
                return Err(error);
            }
            driver.end();
            return Ok(true);
        }
        if !prepare {
            return Ok(false);
        }
        let result = self.session(thread, slot, &driver.lifetime).await.map(drop);
        if let Err(error) = result
            && !driver.lifetime.is_cancelled()
        {
            tracing::warn!(%thread, %error, "ACP thread setup failed");
            let updates = Arc::clone(&self.updates);
            self.db
                .write_tx(move |tx| {
                    exec(
                        tx,
                        "UPDATE threads SET setup_failed = 1 WHERE id = ?1",
                        [thread.into()],
                    )?;
                    let (current, _) = store::lifecycle(tx, thread)?;
                    let next = on_run_end(&current, RunStatus::Failed, updates.is_watched(thread));
                    store::save_lifecycle(tx, thread, &next)
                })
                .await?;
            self.relist(thread);
        }
        Ok(true)
    }
}
