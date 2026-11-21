use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex as SyncMutex},
    time::Duration,
};

use toasty::Executor;
use tokio::{
    sync::{Mutex, Notify, watch},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;

use super::{
    Workspace, attachments,
    runs::{self, Claimed, Settlement},
    sql::{exec, row},
    store::{self, Binding},
    titles,
};
use crate::{
    acp::{
        Catalog, Desired, Outcome, Prompt, Session, SessionSnapshot, StopReason, stop_reason_name,
        transcript::{Output, Transcript},
    },
    wire::{LiveTurn, OptionValue, RpcError, RunEnd, RunId, RunStatus, ToolCallStatus},
};

/// ACP sends no stop signal for idle output. Allow four seconds between
/// updates once the continuation has no live tools.
const CONTINUATION_QUIET: Duration = Duration::from_secs(4);

pub(super) struct Driver {
    pub(super) session: Mutex<Option<Session>>,
    pub(super) lifetime: CancellationToken,
    phase: SyncMutex<Worker>,
    queued: Notify,
}

#[derive(Default)]
enum Worker {
    #[default]
    Idle,
    Scheduled,
    Running {
        run: u64,
        cancel: CancellationToken,
        steer: CancellationToken,
    },
}

impl Driver {
    fn new(lifetime: CancellationToken) -> Self {
        Self {
            session: Mutex::default(),
            lifetime,
            phase: SyncMutex::default(),
            queued: Notify::new(),
        }
    }

    pub(super) fn cancel(&self, run: u64) {
        if let Worker::Running {
            run: active,
            cancel,
            ..
        } = &*self.phase.lock().expect("worker phase mutex poisoned")
            && *active == run
        {
            cancel.cancel();
        }
    }

    pub(super) fn steer(&self, run: u64) {
        if let Worker::Running {
            run: active, steer, ..
        } = &*self.phase.lock().expect("worker phase mutex poisoned")
            && *active == run
        {
            steer.cancel();
        }
    }

    fn is_current(&self, run: u64) -> bool {
        matches!(&*self.phase.lock().expect("worker phase mutex poisoned"), Worker::Running { run: active, .. } if *active == run)
    }
}

impl Workspace {
    /// Called under the DB lock on thread queries, so deletion cannot race
    /// insertion of a new driver.
    pub(super) fn driver(&self, thread: u64) -> Arc<Driver> {
        let mut state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        Arc::clone(
            state
                .threads
                .entry(thread)
                .or_insert_with(|| Arc::new(Driver::new(self.closing.child_token()))),
        )
    }

    pub(super) fn kick(self: &Arc<Self>, thread: u64, prepare: bool) {
        let mut state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        if self.closing.is_cancelled() {
            return;
        }
        let driver = Arc::clone(
            state
                .threads
                .entry(thread)
                .or_insert_with(|| Arc::new(Driver::new(self.closing.child_token()))),
        );
        let mut slot = driver.phase.lock().expect("worker phase mutex poisoned");
        if !matches!(*slot, Worker::Idle) {
            driver.queued.notify_one();
            return;
        }
        *slot = Worker::Scheduled;
        drop(slot);
        let workspace = Arc::clone(self);
        self.jobs.spawn(async move {
            if let Err(error) = workspace.worker(thread, &driver, prepare).await {
                *driver.phase.lock().expect("worker phase mutex poisoned") = Worker::Idle;
                tracing::error!(thread, %error, "ACP scheduler failed");
            }
        });
        drop(state);
    }

    async fn worker(
        self: &Arc<Self>,
        thread: u64,
        driver: &Arc<Driver>,
        mut prepare: bool,
    ) -> Result<(), RpcError> {
        loop {
            let cancel = driver.lifetime.child_token();
            let steer = CancellationToken::new();
            {
                let mut slot = driver.session.lock().await;
                if let Some(session) = slot.as_mut() {
                    self.continue_idle(thread, driver, session, &cancel, &steer)
                        .await?;
                }
            }
            let claim = {
                let mut db = self.db.lock().await;
                if driver.lifetime.is_cancelled() {
                    return Ok(());
                }
                let mut tx = db.transaction().await?;
                let claim = runs::claim(&mut tx, thread).await?;
                tx.commit().await?;
                let mut phase = driver.phase.lock().expect("worker phase mutex poisoned");
                if let Some(claim) = &claim {
                    *phase = Worker::Running {
                        run: claim.id,
                        cancel: cancel.clone(),
                        steer: steer.clone(),
                    };
                } else if !prepare {
                    *phase = Worker::Idle;
                    return Ok(());
                }
                drop(phase);
                drop(db);
                claim
            };
            if let Some(claim) = claim {
                self.changed(Some(thread));
                let run = claim.id;
                let checkpoint = self.start_checkpoint(thread, run).await;
                let mut slot = driver.session.lock().await;
                let result = self
                    .prompt(thread, driver, &mut slot, claim, &cancel, &steer)
                    .await;
                let settlement = settle_outcome(result, &self.closing, &cancel, &steer);
                if settlement.status == RunStatus::Interrupted {
                    slot.take();
                }
                drop(slot);
                let changed_files = if let Some(checkpoint) = checkpoint {
                    self.end_checkpoint(run, checkpoint).await
                } else {
                    None
                };
                if let Err(error) = self
                    .finish(thread, run, settlement, changed_files, &cancel)
                    .await
                {
                    self.abandon(thread, run, &error).await;
                    return Err(error);
                }
            } else {
                let mut slot = driver.session.lock().await;
                let result = self
                    .session(thread, driver, &mut slot, &cancel)
                    .await
                    .map(drop);
                drop(slot);
                if let Err(error) = result
                    && !cancel.is_cancelled()
                {
                    tracing::warn!(thread, %error, "ACP thread setup failed");
                    exec(
                        &mut *self.db.lock().await,
                        "UPDATE threads SET setup_failed = 1 WHERE id = ?1",
                        [thread.into()],
                    )
                    .await?;
                    self.relist(thread);
                }
            }
            prepare = false;
        }
    }

    /// Persist buffered autonomous output before claiming queued input. While
    /// a continuation is running, queued input takes priority once open tools
    /// finish. Content racing the next prompt is folded into that prompt's
    /// transcript, with patches still applied to the original tool entries.
    #[expect(
        clippy::too_many_lines,
        reason = "autonomous continuation coordinates persistence, open-tool tracking, and shutdown"
    )]
    async fn continue_idle(
        &self,
        thread: u64,
        driver: &Driver,
        session: &mut Session,
        cancel: &CancellationToken,
        steer: &CancellationToken,
    ) -> Result<(), RpcError> {
        let pending = session.idle_updates();
        if pending.is_empty() || cancel.is_cancelled() {
            return Ok(());
        }
        let (run, entry) = {
            let mut db = self.db.lock().await;
            if cancel.is_cancelled() {
                return Ok(());
            }
            let mut tx = db.transaction().await?;
            let (run, entry) = runs::begin_unprompted(&mut tx, thread).await?;
            tx.commit().await?;
            *driver.phase.lock().expect("worker phase mutex poisoned") = Worker::Running {
                run,
                cancel: cancel.clone(),
                steer: steer.clone(),
            };
            drop(db);
            (run, entry)
        };
        let checkpoint = self.start_checkpoint(thread, run).await;
        let mut transcript = Transcript::new(entry.to_string(), Arc::default());
        let result = async {
            let mut pending = pending;
            let mut open_patches = HashSet::new();
            loop {
                let entries = transcript.apply(std::mem::take(&mut pending));
                let patches = self
                    .publish(
                        thread,
                        run,
                        session.id(),
                        entries,
                        Some(transcript.live()),
                        session.take_snapshot(),
                    )
                    .await?;
                for (id, status) in patches {
                    if matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress) {
                        open_patches.insert(id);
                    } else {
                        open_patches.remove(&id);
                    }
                }
                let deadline = Instant::now() + CONTINUATION_QUIET;
                loop {
                    let idle = !transcript.has_open_tools() && open_patches.is_empty();
                    if !session.is_alive()
                        || (idle && runs::queued(&mut *self.db.lock().await, thread).await?)
                    {
                        break;
                    }
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        () = driver.queued.notified() => {},
                        update = session.next_idle_update() => {
                            if let Some(update) = update {
                                pending = vec![update];
                                pending.extend(session.idle_updates());
                                break;
                            }
                            break;
                        }
                        () = sleep_until(deadline), if idle => break,
                    }
                }
                if pending.is_empty() {
                    break;
                }
            }
            self.publish(thread, run, session.id(), transcript.finish(), None, None)
                .await?;
            let changed_files = if let Some(checkpoint) = checkpoint {
                self.end_checkpoint(run, checkpoint).await
            } else {
                None
            };
            self.finish(
                thread,
                run,
                Settlement {
                    status: RunStatus::Completed,
                    end: RunEnd {
                        run: None,
                        changed_files: None,
                        stop_reason: None,
                        answer: Some(transcript.answer().to_owned()),
                        error: None,
                    },
                },
                changed_files,
                cancel,
            )
            .await
        }
        .await;
        if let Err(error) = &result {
            self.abandon(thread, run, error).await;
        }
        result
    }

    async fn finish(
        &self,
        thread: u64,
        run: u64,
        mut settlement: Settlement,
        changed_files: Option<usize>,
        cancel: &CancellationToken,
    ) -> Result<(), RpcError> {
        let mut db = self.db.lock().await;
        if runs::status(&mut *db, thread, run).await? != Some(RunStatus::Running) {
            return Ok(());
        }
        if self.closing.is_cancelled() {
            settlement = Settlement::stopped(
                RunStatus::Interrupted,
                "interrupted",
                Some(runs::INTERRUPTED),
            );
        } else if cancel.is_cancelled() {
            settlement = Settlement::stopped(RunStatus::Cancelled, "cancelled", None);
        }
        let mut tx = db.transaction().await?;
        settlement.end.changed_files = changed_files;
        runs::settle(&mut tx, thread, run, settlement, self.unread(thread)).await?;
        tx.commit().await?;
        self.transient
            .lock()
            .expect("workspace runtime mutex poisoned")
            .live
            .remove(&thread);
        drop(db);
        self.settled(thread);
        Ok(())
    }

    /// Best-effort fallback when [`Self::finish`] fails: a run left `running`
    /// would make every later `claim` on the thread violate the `runs_active`
    /// index, so mark it failed with a bare update that appends no entry.
    async fn abandon(&self, thread: u64, run: u64, error: &RpcError) {
        let failed = exec(
            &mut *self.db.lock().await,
            "UPDATE runs SET status = 'failed', error = ?2 WHERE id = ?1 AND status = 'running'",
            [
                run.into(),
                format!("settling the run failed: {error}").into(),
            ],
        )
        .await;
        if let Err(error) = failed {
            tracing::error!(thread, run, %error, "could not fail an unsettled run");
        }
        self.transient
            .lock()
            .expect("workspace runtime mutex poisoned")
            .live
            .remove(&thread);
        self.settled(thread);
    }

    async fn prompt(
        self: &Arc<Self>,
        thread: u64,
        driver: &Arc<Driver>,
        slot: &mut Option<Session>,
        claim: Claimed,
        cancel: &CancellationToken,
        steer: &CancellationToken,
    ) -> anyhow::Result<Outcome> {
        let Bound { session, .. } = self.session(thread, driver, slot, cancel).await?;
        let files = attachments::materialize(self, thread, claim.id, claim.attachments).await?;
        let text = if let Some(author) = claim
            .origin
            .as_ref()
            .and_then(|origin| origin.author.as_deref())
        {
            std::borrow::Cow::Owned(format!("{author}: {}", claim.text))
        } else {
            std::borrow::Cow::Borrowed(claim.text.as_str())
        };
        let id = session.id().to_owned();
        let id = id.as_str();
        let (seed, text) = self.seed_prompt(thread, text).await?;
        let outcome = session
            .prompt(
                Prompt {
                    text: &text,
                    turn: claim.entry.to_string(),
                    run: RunId(claim.id),
                    attachments: &files,
                    cancel: cancel.clone(),
                    steer: steer.clone(),
                },
                |entries, live, snapshot| async move {
                    self.publish(thread, claim.id, id, entries, live, snapshot)
                        .await
                        .map(drop)
                        .map_err(anyhow::Error::from)
                },
            )
            .await?;
        if seed {
            super::rewind::clear_seed(&mut *self.db.lock().await, thread).await?;
        }
        Ok(outcome)
    }

    async fn publish(
        &self,
        thread: u64,
        run: u64,
        session: &str,
        entries: Vec<Output>,
        live: Option<LiveTurn>,
        snapshot: Option<SessionSnapshot>,
    ) -> Result<Vec<(String, ToolCallStatus)>, RpcError> {
        let mut db = self.db.lock().await;
        let current = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned")
            .threads
            .get(&thread)
            .is_some_and(|driver| driver.is_current(run));
        let mut patches = Vec::new();
        if !current {
            return Ok(patches);
        }
        let mut relist = false;
        if !entries.is_empty() || snapshot.is_some() {
            let mut tx = db.transaction().await?;
            if let Some(snapshot) = snapshot {
                relist = persist_snapshot(&mut tx, thread, snapshot, None).await?;
            }
            for entry in entries {
                if let Some(patch) = store::publish(&mut tx, thread, session, entry).await? {
                    patches.push(patch);
                }
            }
            tx.commit().await?;
        }
        let mut state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        if let Some(live) = live {
            state.live.insert(thread, live);
        } else {
            state.live.remove(&thread);
        }
        drop(state);
        drop(db);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(patches)
    }

    async fn session<'slot>(
        self: &Arc<Self>,
        thread: u64,
        driver: &Arc<Driver>,
        slot: &'slot mut Option<Session>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<Bound<'slot>> {
        let Binding {
            harness,
            cwd,
            session,
            ..
        } = {
            let mut db = self.db.lock().await;
            store::binding(&mut *db, thread).await?
        };
        if slot.as_ref().is_some_and(Session::is_alive) {
            return Ok(Bound {
                session: slot.as_mut().expect("checked live session"),
                harness,
                desired: session.desired,
            });
        }
        slot.take();
        let (_, config) = self.harness(Some(&harness))?;
        let endpoint = self.mcp.endpoint(thread);
        let running = tokio::select! {
            biased;
            () = cancel.cancelled() => anyhow::bail!("ACP session setup cancelled"),
            result = Session::open(config, Path::new(&cwd), session.session_id.as_deref(), Some(&endpoint)) => result?,
        };
        if session.session_id.as_deref() != Some(running.id()) {
            exec(
                &mut *self.db.lock().await,
                "UPDATE threads SET session = json_set(session, '$.session_id', ?2) WHERE id = ?1",
                [thread.into(), running.id().into()],
            )
            .await?;
        }
        let desired = tokio::select! {
            biased;
            () = cancel.cancelled() => anyhow::bail!("ACP session setup cancelled"),
            result = running.restore(&session.desired, session.selection.as_ref()) => result?,
        };
        let updates = running.watch();
        let relist = {
            let mut db = self.db.lock().await;
            let mut tx = db.transaction().await?;
            let snapshot = running.snapshot();
            update_catalog(&mut tx, &harness, &snapshot).await?;
            let relist = persist_snapshot(&mut tx, thread, snapshot, Some(&desired)).await?;
            tx.commit().await?;
            drop(db);
            relist
        };
        let session_ref = slot.insert(running);
        self.observe(thread, driver, updates);
        self.kick(thread, false);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(Bound {
            session: session_ref,
            harness,
            desired,
        })
    }

    pub(super) async fn configure(
        self: &Arc<Self>,
        thread: u64,
        option: &str,
        value: OptionValue,
    ) -> Result<(), RpcError> {
        let driver = {
            let mut db = self.db.lock().await;
            store::require(&mut *db, thread).await?;
            let driver = self.driver(thread);
            drop(db);
            driver
        };
        let cancel = &driver.lifetime;
        let mut slot = driver.session.lock().await;
        let Bound {
            session,
            harness,
            desired,
        } = self.session(thread, &driver, &mut slot, cancel).await?;
        let desired = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(RpcError::unavailable("ACP configuration cancelled")),
            result = session.configure(option, value, &desired) => result.map_err(|error| RpcError::invalid(error.to_string()))?,
        };
        let snapshot = session.snapshot();
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        update_catalog(&mut tx, &harness, &snapshot).await?;
        let relist = persist_snapshot(&mut tx, thread, snapshot, Some(&desired)).await?;
        tx.commit().await?;
        drop(db);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
        }
        Ok(())
    }

    fn observe(
        self: &Arc<Self>,
        thread: u64,
        driver: &Arc<Driver>,
        mut updates: watch::Receiver<()>,
    ) {
        let _state = self
            .transient
            .lock()
            .expect("workspace runtime mutex poisoned");
        if self.closing.is_cancelled() {
            return;
        }
        let workspace = Arc::clone(self);
        let driver = Arc::clone(driver);
        self.jobs.spawn(async move {
            loop {
                tokio::select! {
                    () = driver.lifetime.cancelled() => return,
                    result = updates.changed() => if result.is_err() { return; },
                }
                let slot = driver.session.lock().await;
                let Some(session) = slot.as_ref() else {
                    return;
                };
                if updates.has_changed().is_err() {
                    return;
                }
                updates.borrow_and_update();
                let snapshot = session.snapshot();
                let persisted =
                    persist_snapshot(&mut *workspace.db.lock().await, thread, snapshot, None).await;
                drop(slot);
                workspace.kick(thread, false);
                match persisted {
                    Ok(true) => workspace.relist(thread),
                    Ok(false) => workspace.changed(Some(thread)),
                    Err(error) => {
                        tracing::error!(thread, %error, "saving ACP session update");
                        return;
                    }
                }
            }
        });
    }
}

struct Bound<'slot> {
    session: &'slot mut Session,
    harness: String,
    desired: Desired,
}

fn settle_outcome(
    result: anyhow::Result<Outcome>,
    closing: &CancellationToken,
    cancel: &CancellationToken,
    steer: &CancellationToken,
) -> Settlement {
    if closing.is_cancelled() {
        return Settlement::stopped(
            RunStatus::Interrupted,
            "interrupted",
            Some(runs::INTERRUPTED),
        );
    }
    if cancel.is_cancelled() {
        return Settlement::stopped(RunStatus::Cancelled, "cancelled", None);
    }
    if steer.is_cancelled()
        && let Ok(Outcome {
            answer,
            stop_reason: StopReason::Cancelled,
        }) = result
    {
        let mut settlement = Settlement::stopped(RunStatus::Cancelled, "steered", None);
        if !answer.is_empty() {
            settlement.end.answer = Some(answer);
        }
        return settlement;
    }
    outcome(result)
}

fn outcome(outcome: anyhow::Result<Outcome>) -> Settlement {
    match outcome {
        Err(error) => Settlement::failed(error.to_string()),
        Ok(Outcome {
            answer,
            stop_reason,
        }) => {
            let (status, error) = match stop_reason {
                StopReason::Cancelled => (RunStatus::Cancelled, None),
                StopReason::Refusal => (
                    RunStatus::Failed,
                    Some("ACP runner refused the prompt".into()),
                ),
                _ => (RunStatus::Completed, None),
            };
            Settlement {
                status,
                end: RunEnd {
                    run: None,
                    changed_files: None,
                    stop_reason: Some(stop_reason_name(stop_reason)),
                    answer: Some(answer),
                    error,
                },
            }
        }
    }
}

pub(super) async fn catalog(
    db: &mut dyn Executor,
    name: &str,
) -> Result<Option<Catalog>, RpcError> {
    row(
        db,
        "SELECT data FROM harness_catalogs WHERE name = ?1",
        [name.into()],
    )
    .await?
    .map(|mut row| store::from_json(&row.str(0)?))
    .transpose()
}

pub(super) async fn save_catalog(
    db: &mut dyn Executor,
    name: &str,
    catalog: &Catalog,
) -> Result<(), RpcError> {
    exec(db, "INSERT INTO harness_catalogs (name, data) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET data = excluded.data WHERE data <> excluded.data",
        [name.into(), store::json(catalog)?.into()]).await?;
    Ok(())
}

async fn update_catalog(
    db: &mut dyn Executor,
    name: &str,
    snapshot: &SessionSnapshot,
) -> Result<(), RpcError> {
    let stored = catalog(db, name).await?;
    if let Some(updated) = Catalog::merge(stored.as_ref(), &snapshot.options, &snapshot.opened) {
        save_catalog(db, name, &updated).await?;
    }
    Ok(())
}

async fn persist_snapshot(
    db: &mut dyn Executor,
    thread: u64,
    snapshot: SessionSnapshot,
    desired: Option<&Desired>,
) -> Result<bool, RpcError> {
    let title = snapshot
        .title
        .as_deref()
        .map(titles::from_prompt)
        .filter(|title| !title.is_empty());
    let model = option_label(&snapshot.options, "model");
    let thinking = option_label(&snapshot.options, "thought_level");
    let Some(mut prior) = row(
        db,
        "SELECT title, user_title, model, thinking, setup_failed FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    else {
        return Ok(false);
    };
    let prior_title = prior.str(0)?;
    let relist = (!prior.flag(1)? && title.as_ref().is_some_and(|title| *title != prior_title))
        || prior.opt_str(2)?.as_deref() != model
        || prior.opt_str(3)?.as_deref() != thinking
        || prior.flag(4)?;
    exec(db, "UPDATE threads SET
          session = json_set(session, '$.session_id', ?2, '$.options', json(?3), '$.commands', json(?4), '$.images', json(?5), '$.usage', json(?6)),
          title = CASE WHEN user_title = 0 THEN coalesce(?7, title) ELSE title END,
          model = ?8, thinking = ?9, setup_failed = 0 WHERE id = ?1", [
        thread.into(), snapshot.session_id.into(), store::json(&snapshot.options)?.into(), store::json(&snapshot.commands)?.into(),
        if snapshot.images { "true" } else { "false" }.into(), store::json(&snapshot.usage)?.into(), title.into(), model.into(), thinking.into(),
    ]).await?;
    if let Some(desired) = desired {
        exec(db, "UPDATE threads SET session = json_set(session, '$.desired', json(?2), '$.selection', NULL) WHERE id = ?1",
            [thread.into(), store::json(desired)?.into()]).await?;
    }
    Ok(relist)
}

fn option_label<'a>(options: &'a [crate::wire::ConfigOption], category: &str) -> Option<&'a str> {
    let option = options
        .iter()
        .find(|option| option.category.as_deref() == Some(category))?;
    let OptionValue::Select(value) = &option.value else {
        return None;
    };
    Some(
        option
            .choices
            .iter()
            .find(|item| &item.value == value)
            .map_or(value.as_str(), |item| item.name.as_str()),
    )
}
