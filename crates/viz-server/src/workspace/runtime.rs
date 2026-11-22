//! ACP effect interpreter for claimed prompts and autonomous continuations.
//! The worker supplies the active turn and session; the interpreter commits
//! outputs and settlement before publishing any workspace change.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;
use viz_core::{
    handoff,
    runs::{EndStatus, Settlement},
};
use viz_protocol::ThreadId;

use super::{
    Workspace, attachments,
    runs::{self, Claimed},
    sessions::{Bound, SessionSlot, persist_snapshot},
    sql::{Connection, exec, rows},
    store,
    worker::{Driver, Turn},
};
use crate::{
    acp::{
        Outcome, Prompt, Session, SessionSnapshot, StopReason, stop_reason_name,
        transcript::{Output, Transcript},
    },
    error::Error,
    wire::{LiveTurn, RunEnd, RunId, RunStatus, ToolCallStatus},
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
        thread: u64,
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
            let turn = driver.begin(RunId(run));
            drop(db);
            (turn, entry)
        };
        let run = turn.run.0;
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
                Ok::<_, Error>(Settlement {
                    status: EndStatus::Completed,
                    end: RunEnd {
                        stop_reason: None,
                        answer: Some(transcript.answer().to_owned()),
                        error: None,
                    },
                })
            })
            .await;
        let result = match result {
            Ok(settlement) => self.finish(thread, driver, &turn, settlement).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &result {
            self.abandon(thread, run, error).await;
        }
        driver.end();
        result
    }

    pub(super) async fn finish(
        &self,
        thread: u64,
        driver: &Driver,
        turn: &Turn,
        mut settlement: Settlement,
    ) -> Result<(), Error> {
        let run = turn.run.0;
        let mut db = self.db.lock().await;
        if runs::status(&mut db, thread, run).await? != Some(RunStatus::Running) {
            return Ok(());
        }
        if self.closing.is_cancelled() {
            settlement = Settlement::interrupted();
        } else if driver.stopping(turn.run) {
            settlement = Settlement::stopped(EndStatus::Cancelled, "cancelled", None);
        }
        let mut tx = db.transaction().await?;
        runs::settle(&mut tx, thread, run, settlement, self.unread(thread)).await?;
        tx.commit().await?;
        self.updates.live(thread, None);
        drop(db);
        self.settled(thread);
        Ok(())
    }

    /// Best-effort fallback when [`Self::finish`] fails: a run left `running`
    /// would make every later `claim` on the thread violate the `runs_active`
    /// index, so mark it failed with a bare update that appends no entry.
    pub(super) async fn abandon(&self, thread: u64, run: u64, error: &Error) {
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
        self.updates.live(thread, None);
        self.settled(thread);
    }

    pub(super) async fn prompt(
        &self,
        thread: u64,
        slot: &mut SessionSlot,
        claim: Claimed,
        turn: &Turn,
    ) -> anyhow::Result<Outcome> {
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
                Some(from) => Some(handoff_note(&mut db, thread, &binding.cwd, &from).await?),
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
                    run: RunId(claim.id),
                    attachments: &files,
                    cancel: turn.cancel.clone(),
                    steer: turn.steer.clone(),
                },
                |entries, live, snapshot| async move {
                    self.publish(thread, claim.id, id, entries, live, snapshot)
                        .await
                        .map(drop)
                        .map_err(anyhow::Error::from)
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
        outcome
    }

    async fn publish(
        &self,
        thread: u64,
        run: u64,
        session: &str,
        entries: Vec<Output>,
        live: Option<LiveTurn>,
        snapshot: Option<SessionSnapshot>,
    ) -> Result<Vec<(String, ToolCallStatus)>, Error> {
        let mut db = self.db.lock().await;
        let current = self
            .workers
            .get(ThreadId(thread))
            .is_some_and(|driver| driver.is_current(RunId(run)));
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

pub(super) fn settle_outcome(
    result: anyhow::Result<Outcome>,
    closing: &CancellationToken,
    cancel: &CancellationToken,
    steer: &CancellationToken,
) -> Settlement {
    if closing.is_cancelled() {
        return Settlement::interrupted();
    }
    if cancel.is_cancelled() {
        return Settlement::stopped(EndStatus::Cancelled, "cancelled", None);
    }
    if steer.is_cancelled()
        && let Ok(Outcome {
            answer,
            stop_reason: StopReason::Cancelled,
        }) = result
    {
        let mut settlement = Settlement::stopped(EndStatus::Cancelled, "steered", None);
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
                StopReason::Cancelled => (EndStatus::Cancelled, None),
                StopReason::Refusal => (
                    EndStatus::Failed,
                    Some("ACP runner refused the prompt".into()),
                ),
                _ => (EndStatus::Completed, None),
            };
            Settlement {
                status,
                end: RunEnd {
                    stop_reason: Some(stop_reason_name(stop_reason)),
                    answer: Some(answer),
                    error,
                },
            }
        }
    }
}

/// Load the durable turns for the first prompt after a move.
async fn handoff_note(
    db: &mut Connection,
    thread: u64,
    cwd: &str,
    from: &str,
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
    Ok(handoff::note(
        cwd,
        from,
        turns
            .iter()
            .map(|(prompt, answer)| (prompt.as_str(), answer.as_str())),
    ))
}
