//! ACP effect interpreter for claimed prompts and autonomous continuations.
//! The worker supplies the active turn and session; the interpreter commits
//! outputs and settlement before publishing any workspace change.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::time::{Instant, sleep_until};
use viz_core::{
    handoff::{self, Reason},
    runs::{self as decisions, Settlement, Signals, Stop, TurnEnd},
};
use viz_protocol::{CheckpointEdge, ThreadId, Withdrawal};

use super::{
    Workspace, attachments,
    runs::{self, Claimed},
    sessions::{Bound, SessionSlot, persist_snapshot},
    sql::{self, Conn, exec, rows},
    store::{self, Handoff},
    worker::{Driver, Turn},
};
use crate::{
    acp::{
        Outcome, Output, Prompt, Session, SessionSnapshot, StopReason, Transcript, stop_reason_name,
    },
    error::Error,
    wire::{RunId, RunStatus, ToolCallStatus},
};

/// ACP sends no stop signal for idle output. Allow four seconds between
/// updates once the continuation has no live tools.
const CONTINUATION_QUIET: Duration = Duration::from_secs(4);

impl Workspace {
    /// Persist buffered autonomous output before claiming queued input. While
    /// a continuation is running, queued input takes priority once open tools
    /// finish. Content racing the next prompt is folded into that prompt's
    /// transcript, with patches still applied to the original tool entries.
    /// The harness acts on its own, so the continuation occupies its checkout
    /// only when no exclusive Git action already holds it.
    pub(super) async fn continue_idle(
        &self,
        thread: ThreadId,
        driver: &Arc<Driver>,
        session: &mut Session,
    ) -> Result<(), Error> {
        let pending = session.idle_updates();
        if pending.is_empty() || driver.lifetime.is_cancelled() {
            return Ok(());
        }
        let root = self.turn_root(thread).await?;
        let driver_for_write = Arc::clone(driver);
        let reservations = Arc::clone(&self.reservations);
        let Some((turn, entry, _occupancy)) = self
            .db
            .write(move |conn| {
                if driver_for_write.lifetime.is_cancelled() {
                    return Ok(None);
                }
                let occupancy = reservations.occupy(root.as_deref());
                let tx = sql::immediate(conn)?;
                let (run, entry) = runs::begin_unprompted(&tx, thread)?;
                tx.commit()?;
                let turn = driver_for_write.begin(run);
                Ok(Some((turn, entry, occupancy)))
            })
            .await?
        else {
            return Ok(());
        };
        let run = turn.run;
        self.checkpoint(thread, run, CheckpointEdge::Before).await;
        let cancel = &turn.cancel;
        let mut transcript = Transcript::new(entry.to_string(), Arc::default());
        let result = driver
            .interpret(&turn, async {
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
                            || (idle
                                && self.db.read(move |conn| runs::queued(conn, thread)).await?)
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
                Ok::<_, Error>(TurnEnd {
                    answer: transcript.answer().to_owned(),
                    stop: Stop::Finished,
                    reason: None,
                    usage: None,
                })
            })
            .await;
        let result = match result {
            Ok(end) => self.finish(thread, driver, &turn, Ok(end)).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &result {
            self.abandon(thread, run, error).await;
        }
        driver.end();
        result
    }

    /// Settle the turn's run unless something else already has. Either way
    /// the turn's live view and request waiters end with it.
    pub(super) async fn finish(
        &self,
        thread: ThreadId,
        driver: &Arc<Driver>,
        turn: &Turn,
        end: Result<TurnEnd, String>,
    ) -> Result<(), Error> {
        let run = turn.run;
        self.checkpoint(thread, run, CheckpointEdge::After).await;
        let driver = Arc::clone(driver);
        let closing = self.closing.clone();
        let steered = turn.steer.clone();
        let requests = Arc::clone(&self.requests);
        let updates = Arc::clone(&self.updates);
        self.db
            .write(move |conn| {
                if runs::status(conn, thread, run)? != Some(RunStatus::Running) {
                    requests.release_run(run);
                    updates.live(thread, None);
                    return Ok(());
                }
                let signals = Signals {
                    closing: closing.is_cancelled(),
                    stopped: driver.stopping(run),
                    steered: steered.is_cancelled(),
                };
                let settlement = decisions::settle(signals, end);
                let tx = sql::immediate(conn)?;
                runs::settle(&tx, thread, run, settlement, updates.is_watched(thread))?;
                tx.commit()?;
                requests.release_run(run);
                updates.live(thread, None);
                Ok(())
            })
            .await?;
        self.settled(thread);
        Ok(())
    }

    /// Fallback when [`Self::finish`] fails: a run left `running` would make
    /// every later `claim` on the thread violate the `runs_active` index.
    /// Settles it as orphaned, with its end entry, lifecycle, and follow-ups,
    /// like any other run; only if that fails too is it marked failed with a
    /// bare update.
    pub(super) async fn abandon(&self, thread: ThreadId, run: RunId, error: &Error) {
        self.requests.release_run(run);
        let message = format!("settling the run failed: {error}");
        let updates = Arc::clone(&self.updates);
        let failed = self
            .db
            .write(move |conn| {
                let settled = (|| -> Result<(), Error> {
                    if runs::status(conn, thread, run)? != Some(RunStatus::Running) {
                        return Ok(());
                    }
                    let tx = sql::immediate(conn)?;
                    runs::settle(
                        &tx,
                        thread,
                        run,
                        Settlement::orphaned(),
                        updates.is_watched(thread),
                    )?;
                    tx.commit()?;
                    Ok(())
                })();
                let outcome = match settled {
                    Ok(()) => Ok(()),
                    Err(settling) => {
                        tracing::warn!(%thread, %run, %settling, "settling an abandoned run");
                        match exec(
                            conn,
                            "UPDATE runs SET status = 'failed', error = ?2 WHERE id = ?1 AND status = 'running'",
                            [run.into(), message.into()],
                        ) {
                            Ok(_) => store::withdraw_requests(conn, Some(run), Withdrawal::TurnEnded)
                                .map(drop),
                            Err(error) => Err(error),
                        }
                    }
                };
                updates.live(thread, None);
                outcome
            })
            .await;
        if let Err(error) = failed {
            tracing::error!(%thread, %run, %error, "could not fail an unsettled run");
        }
        self.settled(thread);
        self.relist(thread);
    }

    pub(super) async fn prompt(
        &self,
        thread: ThreadId,
        slot: &mut SessionSlot,
        claim: Claimed,
        turn: &Turn,
    ) -> Result<Outcome, Error> {
        let Bound { session, .. } = self.session(thread, slot, &turn.cancel).await?;
        let files = attachments::materialize(self, thread, claim.id, claim.attachments).await?;
        let mut text = if let Some(author) = claim
            .origin
            .as_ref()
            .and_then(|origin| origin.author.as_deref())
        {
            std::borrow::Cow::Owned(format!("{author}: {}", claim.text))
        } else {
            std::borrow::Cow::Borrowed(claim.text.as_str())
        };
        let (cwd, handoff) = self
            .db
            .read_snapshot(move |tx| {
                let binding = store::binding(tx, thread)?;
                let note = match binding.session.handoff {
                    Some(from) => Some(handoff_note(tx, thread, &binding.checkout.cwd, &from)?),
                    None => None,
                };
                Ok((binding.checkout.cwd, note))
            })
            .await?;
        if let Some(note) = &handoff {
            text = std::borrow::Cow::Owned(format!("{note}{text}"));
        }
        let context = viz_acp::context_blocks(&claim.context, std::path::Path::new(&cwd));
        let id = session.id().to_owned();
        let id = id.as_str();
        let outcome = session
            .prompt(
                Prompt {
                    text: &text,
                    turn: claim.entry.to_string(),
                    run: claim.id,
                    context: &context,
                    attachments: &files,
                    cancel: turn.cancel.clone(),
                    steer: turn.steer.clone(),
                },
                |entries, live, snapshot| async move {
                    self.publish(thread, claim.id, id, entries, live, snapshot)
                        .await
                        .map(drop)
                },
            )
            .await;
        if handoff.is_some() && outcome.is_ok() {
            self.db
                .write(move |conn| {
                    exec(
                        conn,
                        "UPDATE threads SET session = json_remove(session, '$.handoff') WHERE id = ?1",
                        [thread.into()],
                    )?;
                    Ok(())
                })
                .await?;
        }
        outcome.map_err(Error::from)
    }

    async fn publish(
        &self,
        thread: ThreadId,
        run: RunId,
        session: &str,
        mut entries: Vec<Output>,
        mut live: Option<viz_acp::LiveTurn>,
        snapshot: Option<SessionSnapshot>,
    ) -> Result<Vec<(String, ToolCallStatus)>, Error> {
        let extracted = attachments::extract_media_blobs(self, &mut entries, live.as_mut()).await?;
        let session = session.to_owned();
        let workers = Arc::clone(&self.workers);
        let updates = Arc::clone(&self.updates);
        let entry_chunks = Arc::clone(&self.entry_chunks);
        let (active, relist, patches) = self
            .db
            .write(move |conn| {
                let current = workers
                    .get(thread)
                    .is_some_and(|driver| driver.is_current(run));
                let mut patches = Vec::new();
                if !current {
                    return Ok((false, false, patches));
                }
                let mut relist = false;
                if !entries.is_empty() || !extracted.is_empty() || snapshot.is_some() {
                    let tx = sql::immediate(conn)?;
                    attachments::link_media_blobs(&tx, thread, &extracted)?;
                    if let Some(snapshot) = snapshot {
                        relist = persist_snapshot(&tx, thread, snapshot, None)?;
                    }
                    for entry in entries {
                        if let Some(patch) =
                            store::publish(&tx, thread, &session, entry, &entry_chunks)?
                        {
                            patches.push(patch);
                        }
                    }
                    tx.commit()?;
                }
                drop(extracted);
                updates.live(thread, live);
                Ok((true, relist, patches))
            })
            .await?;
        if active {
            if relist {
                self.relist(thread);
            } else {
                self.changed(Some(thread));
            }
        }
        Ok(patches)
    }
}

/// What the harness's answer to a prompt says about the turn.
pub(super) fn turn_end(outcome: Result<Outcome, Error>) -> Result<TurnEnd, String> {
    let Outcome {
        answer,
        stop_reason,
        usage,
    } = outcome.map_err(|error| error.to_string())?;
    Ok(TurnEnd {
        answer,
        stop: match stop_reason {
            StopReason::Cancelled => Stop::Cancelled,
            StopReason::Refusal => Stop::Refused,
            _ => Stop::Finished,
        },
        reason: Some(stop_reason_name(stop_reason)),
        usage,
    })
}

/// Load the durable turns for the first prompt after a move.
fn handoff_note(
    db: &Conn,
    thread: ThreadId,
    cwd: &str,
    handoff: &Handoff,
) -> Result<String, Error> {
    let turns = rows(
        db,
        "SELECT text, coalesce(answer, '') FROM runs WHERE thread = ?1 AND kind = 'prompt'
            AND status IN ('completed', 'failed', 'cancelled', 'interrupted') ORDER BY id DESC",
        [thread.into()],
    )?
    .map(|mut row| Ok((row.str(0)?, row.str(1)?)))
    .collect::<Result<Vec<_>, Error>>()?;
    let reason = match handoff {
        Handoff::Moved { from } => Reason::Moved { from, to: cwd },
        Handoff::Forked { .. } => Reason::Forked,
        Handoff::Reopened => Reason::Reopened,
    };
    Ok(handoff::note(
        reason,
        turns
            .iter()
            .map(|(prompt, answer)| (prompt.as_str(), answer.as_str())),
    ))
}
