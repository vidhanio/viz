//! ACP effect interpreter for claimed prompts and autonomous continuations.
//! The worker supplies the active turn and session; the interpreter commits
//! outputs and settlement before publishing any workspace change.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::time::{Instant, sleep_until};
use viz_core::{
    handoff::{self, Reason},
    runs::{self as decisions, Signals, Stop, TurnEnd},
};
use viz_protocol::{CheckpointEdge, ThreadId, Withdrawal};

use super::{
    Workspace, attachments,
    runs::{self, Claimed},
    sessions::{Bound, SessionSlot, persist_snapshot},
    sql::{Connection, exec, rows},
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
    pub(super) async fn continue_idle(
        &self,
        thread: ThreadId,
        driver: &Driver,
        session: &mut Session,
    ) -> Result<(), Error> {
        let pending = session.idle_updates();
        if pending.is_empty() || driver.lifetime.is_cancelled() {
            return Ok(());
        }
        let (turn, entry) = {
            let mut db = self.db.lock().await;
            if driver.lifetime.is_cancelled() {
                return Ok(());
            }
            let mut tx = db.transaction().await?;
            let (run, entry) = runs::begin_unprompted(&mut tx, thread).await?;
            tx.commit().await?;
            let turn = driver.begin(run);
            drop(db);
            (turn, entry)
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

    /// Settle the turn's run unless something else already has.
    pub(super) async fn finish(
        &self,
        thread: ThreadId,
        driver: &Driver,
        turn: &Turn,
        end: Result<TurnEnd, String>,
    ) -> Result<(), Error> {
        let run = turn.run;
        self.checkpoint(thread, run, CheckpointEdge::After).await;
        let mut db = self.db.lock().await;
        if runs::status(&mut db, thread, run).await? != Some(RunStatus::Running) {
            return Ok(());
        }
        let signals = Signals {
            closing: self.closing.is_cancelled(),
            stopped: driver.stopping(run),
            steered: turn.steer.is_cancelled(),
        };
        let settlement = decisions::settle(signals, end);
        let mut tx = db.transaction().await?;
        runs::settle(&mut tx, thread, run, settlement, self.is_watched(thread)).await?;
        tx.commit().await?;
        self.requests.release_run(run);
        self.updates.live(thread, None);
        drop(db);
        self.settled(thread);
        Ok(())
    }

    /// Best-effort fallback when [`Self::finish`] fails: a run left `running`
    /// would make every later `claim` on the thread violate the `runs_active`
    /// index, so mark it failed with a bare update that appends no entry.
    pub(super) async fn abandon(&self, thread: ThreadId, run: RunId, error: &Error) {
        self.requests.release_run(run);
        let mut db = self.db.lock().await;
        let failed = exec(
            &mut db,
            "UPDATE runs SET status = 'failed', error = ?2 WHERE id = ?1 AND status = 'running'",
            [
                run.into(),
                format!("settling the run failed: {error}").into(),
            ],
        )
        .await;
        let failed = match failed {
            Ok(_) => store::withdraw_requests(&mut db, Some(run), Withdrawal::TurnEnded)
                .await
                .map(drop),
            Err(error) => Err(error),
        };
        drop(db);
        if let Err(error) = failed {
            tracing::error!(%thread, %run, %error, "could not fail an unsettled run");
        }
        self.updates.live(thread, None);
        self.settled(thread);
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
        let handoff = {
            let mut db = self.db.lock().await;
            let binding = store::binding(&mut db, thread).await?;
            let note = match binding.session.handoff {
                Some(from) => {
                    Some(handoff_note(&mut db, thread, &binding.checkout.cwd, &from).await?)
                }
                None => None,
            };
            drop(db);
            note
        };
        if let Some(note) = &handoff {
            text = std::borrow::Cow::Owned(format!("{note}{text}"));
        }
        let id = session.id().to_owned();
        let id = id.as_str();
        let outcome = session
            .prompt(
                Prompt {
                    text: &text,
                    turn: claim.entry.to_string(),
                    run: claim.id,
                    context: &[],
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
            exec(
                &mut *self.db.lock().await,
                "UPDATE threads SET session = json_remove(session, '$.handoff') WHERE id = ?1",
                [thread.into()],
            )
            .await?;
        }
        outcome.map_err(Error::from)
    }

    async fn publish(
        &self,
        thread: ThreadId,
        run: RunId,
        session: &str,
        entries: Vec<Output>,
        live: Option<viz_acp::LiveTurn>,
        snapshot: Option<SessionSnapshot>,
    ) -> Result<Vec<(String, ToolCallStatus)>, Error> {
        let mut db = self.db.lock().await;
        let current = self
            .workers
            .get(thread)
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
        self.updates.live(thread, live);
        drop(db);
        if relist {
            self.relist(thread);
        } else {
            self.changed(Some(thread));
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
async fn handoff_note(
    db: &mut Connection,
    thread: ThreadId,
    cwd: &str,
    handoff: &Handoff,
) -> Result<String, Error> {
    let turns = rows(
        db,
        "SELECT text, coalesce(answer, '') FROM runs WHERE thread = ?1 AND kind = 'prompt'
            AND status IN ('completed', 'failed', 'cancelled', 'interrupted') ORDER BY id DESC",
        [thread.into()],
    )
    .await?
    .map(|mut row| Ok((row.str(0)?, row.str(1)?)))
    .collect::<Result<Vec<_>, Error>>()?;
    let reason = match handoff {
        Handoff::Moved { from } => Reason::Moved { from, to: cwd },
        Handoff::Forked => Reason::Forked,
    };
    Ok(handoff::note(
        reason,
        turns
            .iter()
            .map(|(prompt, answer)| (prompt.as_str(), answer.as_str())),
    ))
}
