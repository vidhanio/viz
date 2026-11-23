//! Durable run persistence and the continuation outbox.

use std::collections::HashSet;

use viz_core::{
    lifecycle,
    runs::{self as decisions, Claim, Move, Place, Settlement},
    threads::Patch,
    titles,
};

use super::{
    sql::{Row, exec, row, rows},
    store,
};
use crate::{
    error::Error,
    wire::{EntryId, NewEntry, Origin, PromptAttachment, Run, RunId, RunKind, RunStatus, ThreadId},
    workspace::sql::Connection,
};

pub(super) struct NewRun<'a> {
    pub(super) text: &'a str,
    pub(super) origin: Option<&'a Origin>,
    pub(super) attachments: &'a [PromptAttachment],
    pub(super) request_id: Option<&'a str>,
    /// The entry of a task's result that stands in for a prompt: the input is
    /// the agent's, not the user's.
    pub(super) existing_entry: Option<EntryId>,
    pub(super) place: Place,
}

impl<'a> NewRun<'a> {
    pub(super) const fn new(text: &'a str) -> Self {
        Self {
            text,
            origin: None,
            attachments: &[],
            request_id: None,
            existing_entry: None,
            place: Place::End,
        }
    }
}

pub(super) async fn enqueue(
    db: &mut Connection,
    thread: ThreadId,
    new: NewRun<'_>,
) -> Result<RunId, Error> {
    let origin = new.origin.map(store::json).transpose()?;
    let attachments = (!new.attachments.is_empty())
        .then(|| store::json(new.attachments))
        .transpose()?;
    let position = queue_position(db, thread, new.place).await?;
    let run = row(db, "INSERT INTO runs (thread, status, position, text, origin, attachments, request_id, prompt_entry)
        VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id", [
        thread.into(), position.into(), new.text.into(), origin.into(), attachments.into(),
        new.request_id.into(), new.existing_entry.into(),
    ]).await?.expect("run INSERT RETURNING yielded one row").id(0)?;
    let mut thread_row = row(
        db,
        "SELECT title FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    .ok_or_else(|| Error::NoThread(thread))?;
    let title = titles::enqueue(thread_row.str(0)?, new.text);
    exec(
        db,
        "UPDATE threads SET title = ?2 WHERE id = ?1",
        [thread.into(), title.into()],
    )
    .await?;
    if new.existing_entry.is_some() {
        store::touch(db, thread, lifecycle::on_agent_activity).await?;
    } else {
        store::touch(db, thread, lifecycle::on_input).await?;
        exec(
            db,
            "UPDATE threads SET last_input_at = updated_at WHERE id = ?1",
            [thread.into()],
        )
        .await?;
    }
    Ok(run)
}

pub(super) async fn by_request(
    db: &mut Connection,
    thread: ThreadId,
    request_id: &str,
) -> Result<Option<RunId>, Error> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND request_id = ?2",
        [thread.into(), request_id.into()],
    )
    .await?
    .map(|row| row.id(0))
    .transpose()
}

pub(super) async fn origin(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<Option<Origin>, Error> {
    let Some(mut row) = row(
        db,
        "SELECT origin FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    else {
        return Ok(None);
    };
    row.opt_str(0)?.as_deref().map(store::from_json).transpose()
}

pub(crate) async fn run_from(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<Option<Run>, Error> {
    row(
        db,
        "SELECT id, status, answer, error, kind, usage FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) async fn by_ids(
    db: &mut Connection,
    thread: ThreadId,
    runs: &HashSet<RunId>,
) -> Result<Vec<Run>, Error> {
    if runs.is_empty() {
        return Ok(Vec::new());
    }
    rows(
        db,
        "SELECT id, status, answer, error, kind, usage FROM runs WHERE thread = ?1
         AND id IN (SELECT value FROM json_each(?2))",
        [thread.into(), store::json(runs)?.into()],
    )
    .await?
    .map(|record| decode_run(thread, record))
    .collect()
}

pub(super) async fn latest(db: &mut Connection, thread: ThreadId) -> Result<Option<Run>, Error> {
    store::require(db, thread).await?;
    row(
        db,
        "SELECT id, status, answer, error, kind, usage FROM runs WHERE thread = ?1 ORDER BY id DESC LIMIT 1",
        [thread.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) async fn active_run_from(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<Option<RunId>, Error> {
    store::require(db, thread).await?;
    active(db, thread).await
}

pub(super) async fn active(db: &mut Connection, thread: ThreadId) -> Result<Option<RunId>, Error> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'running'",
        [thread.into()],
    )
    .await?
    .map(|row| row.id(0))
    .transpose()
}

fn decode_run(thread: ThreadId, mut row: Row) -> Result<Run, Error> {
    Ok(Run {
        id: row.id(0)?,
        thread,
        kind: if row.str(4)? == "unprompted" {
            RunKind::Unprompted
        } else {
            RunKind::Prompt
        },
        status: decode_status(&row.str(1)?)?,
        answer: row.opt_str(2)?,
        error: row.opt_str(3)?,
        usage: row
            .opt_str(5)?
            .as_deref()
            .map(store::from_json)
            .transpose()?,
    })
}

pub(super) async fn status(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<Option<RunStatus>, Error> {
    row(
        db,
        "SELECT status FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .map(|mut row| decode_status(&row.str(0)?))
    .transpose()
}

/// The name stored in `runs.status`.
const fn status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
    }
}

pub(super) fn decode_status(value: &str) -> Result<RunStatus, Error> {
    Ok(match value {
        "queued" => RunStatus::Queued,
        "running" => RunStatus::Running,
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        "interrupted" => RunStatus::Interrupted,
        _ => {
            return Err(Error::corrupt(format_args!(
                "unknown persisted run status {value}"
            )));
        }
    })
}

pub(super) struct Claimed {
    pub(super) id: RunId,
    pub(super) entry: EntryId,
    pub(super) text: String,
    pub(super) origin: Option<Origin>,
    pub(super) attachments: Vec<PromptAttachment>,
}

/// What a worker found to do.
pub(super) struct Found {
    pub(super) claimed: Option<Claimed>,
    /// A run nobody was on was settled.
    pub(super) recovered: bool,
}

/// Commit the claim and corresponding prompt together before starting any
/// external operation.
pub(super) async fn claim(db: &mut Connection, thread: ThreadId) -> Result<Found, Error> {
    let active = active(db, thread).await?;
    let next = row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id LIMIT 1",
        [thread.into()],
    )
    .await?
    .map(|row| row.id(0))
    .transpose()?;
    let (id, recovered) = match decisions::claim(active, next) {
        Claim::Nothing => (None, false),
        Claim::Start(id) => (Some(id), false),
        Claim::Orphan(run) => {
            settle(db, thread, run, Settlement::orphaned(), false).await?;
            (next, true)
        }
    };
    let claimed = match id {
        Some(id) => Some(start(db, thread, id).await?),
        None => None,
    };
    Ok(Found { claimed, recovered })
}

async fn start(db: &mut Connection, thread: ThreadId, id: RunId) -> Result<Claimed, Error> {
    let mut row = row(
        db,
        "UPDATE runs SET status = 'running' WHERE id = ?1
        RETURNING text, origin, attachments, prompt_entry",
        [id.into()],
    )
    .await?
    .expect("claimed run exists under the write lease");
    let text = row.str(0)?;
    let origin = row
        .opt_str(1)?
        .as_deref()
        .map(store::from_json)
        .transpose()?;
    let attachments = row
        .opt_str(2)?
        .as_deref()
        .map(store::from_json)
        .transpose()?
        .unwrap_or_default();
    let (entry, text, origin, attachments) = if let Some(entry) = row.opt_id(3)? {
        (entry, text, origin, attachments)
    } else {
        let data = NewEntry::Prompt {
            text,
            origin,
            attachments,
        };
        let entry = store::append(db, thread, &data).await?;
        exec(
            db,
            "UPDATE runs SET prompt_entry = ?2 WHERE id = ?1",
            [id.into(), entry.into()],
        )
        .await?;
        let NewEntry::Prompt {
            text,
            origin,
            attachments,
        } = data
        else {
            unreachable!()
        };
        (entry, text, origin, attachments)
    };
    Ok(Claimed {
        id,
        entry,
        text,
        origin,
        attachments,
    })
}

/// Starts an unprompted run: the agent sent transcript content with no turn
/// open. It is created `running`, with a `continued` entry in place of a
/// prompt, and returns the run and that entry. Its caller holds the worker's
/// slot, so the `runs_active` index cannot conflict.
pub(super) async fn begin_unprompted(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<(RunId, EntryId), Error> {
    let entry = store::append(db, thread, &NewEntry::Continued).await?;
    let run = row(
        db,
        "INSERT INTO runs (thread, kind, status, position, text, prompt_entry)
        VALUES (?1, 'unprompted', 'running', 0, '', ?2) RETURNING id",
        [thread.into(), entry.into()],
    )
    .await?
    .expect("run INSERT RETURNING yielded one row")
    .id(0)?;
    store::touch(db, thread, lifecycle::on_agent_activity).await?;
    Ok((run, entry))
}

/// Whether inputs are waiting on the thread.
pub(super) async fn queued(db: &mut Connection, thread: ThreadId) -> Result<bool, Error> {
    Ok(row(
        db,
        "SELECT 1 FROM runs WHERE thread = ?1 AND status = 'queued' LIMIT 1",
        [thread.into()],
    )
    .await?
    .is_some())
}

pub(super) async fn settle(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
    settlement: Settlement,
    watched: bool,
) -> Result<(), Error> {
    let ended = settlement.status.into();
    let status = status_name(ended);
    let data = NewEntry::Run(settlement.end);
    let entry = store::append(db, thread, &data).await?;
    let NewEntry::Run(end) = data else {
        unreachable!()
    };
    exec(
        db,
        "UPDATE runs SET status = ?2, answer = ?3, error = ?4, end_entry = ?5, usage = ?6 WHERE id = ?1",
        [
            run.into(),
            status.into(),
            end.answer.into(),
            end.error.into(),
            entry.into(),
            end.usage.as_ref().map(store::json).transpose()?.into(),
        ],
    )
    .await?;
    store::touch(db, thread, |current| {
        lifecycle::on_run_end(current, ended, watched)
    })
    .await?;
    exec(
        db,
        "UPDATE threads SET setup_failed = 0 WHERE id = ?1",
        [thread.into()],
    )
    .await?;
    super::follow_ups::record(db, thread, run).await
}

pub(super) async fn recover(db: &mut Connection) -> Result<(), Error> {
    for row in rows(
        db,
        "SELECT id, thread FROM runs WHERE status = 'running'",
        [],
    )
    .await?
    {
        settle(db, row.id(1)?, row.id(0)?, Settlement::interrupted(), false).await?;
    }
    Ok(())
}

async fn queue_position(db: &mut Connection, thread: ThreadId, place: Place) -> Result<i64, Error> {
    let bounds = row(
        db,
        "SELECT min(position), max(position) FROM runs
        WHERE thread = ?1 AND status = 'queued'",
        [thread.into()],
    )
    .await?
    .expect("aggregate query returns a row");
    let bounds = bounds.opt_i64(0)?.zip(bounds.opt_i64(1)?);
    place
        .position(bounds)
        .ok_or_else(|| Error::corrupt("run queue position overflow"))
}

/// The queued runs in dispatch order.
async fn queue(db: &mut Connection, thread: ThreadId) -> Result<Vec<RunId>, Error> {
    rows(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id",
        [thread.into()],
    )
    .await?
    .map(|row| row.id(0))
    .collect()
}

async fn save_order(db: &mut Connection, queue: &[RunId]) -> Result<(), Error> {
    exec(
        db,
        "UPDATE runs AS r SET position = j.key FROM json_each(?1) AS j
         WHERE r.id = j.value AND r.position <> j.key",
        [store::json(queue)?.into()],
    )
    .await?;
    Ok(())
}

async fn cancel_queued(db: &mut Connection, runs: &[RunId], reason: &str) -> Result<(), Error> {
    exec(
        db,
        "UPDATE runs SET status = 'cancelled', error = ?2
         WHERE id IN (SELECT value FROM json_each(?1))",
        [store::json(runs)?.into(), reason.into()],
    )
    .await?;
    Ok(())
}

/// Move a queued run to the front, keeping its identity and payload.
pub(super) async fn promote(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<(), Error> {
    let order = decisions::promote(&queue(db, thread).await?, run)?;
    save_order(db, &order).await
}

pub(super) async fn remove(db: &mut Connection, thread: ThreadId, run: RunId) -> Result<(), Error> {
    let run = decisions::remove(&queue(db, thread).await?, run)?;
    cancel_queued(db, &[run], "removed from queue").await
}

/// Cancel the queued runs a stop covers, and return the running one for the
/// worker to stop.
pub(super) async fn cancel(
    db: &mut Connection,
    thread: ThreadId,
    wanted: Option<RunId>,
) -> Result<Option<RunId>, Error> {
    let cancellation =
        decisions::cancel(queue(db, thread).await?, active(db, thread).await?, wanted);
    cancel_queued(db, &cancellation.queued, "cancelled before dispatch").await?;
    Ok(cancellation.active)
}

pub(super) async fn idle(db: &mut Connection, thread: ThreadId) -> Result<(), Error> {
    let queued = queued(db, thread).await?;
    Ok(decisions::idle(queued, active(db, thread).await?)?)
}

pub(super) async fn update_queue(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
    text: Option<&str>,
    before: Patch<RunId>,
) -> Result<(), Error> {
    let placement = match before {
        Patch::Set(target) => Move::Before(target),
        Patch::Clear => Move::End,
        Patch::Keep => Move::Keep,
    };
    if let Some(order) = decisions::reorder(queue(db, thread).await?, run, placement)? {
        save_order(db, &order).await?;
    }
    if let Some(text) = text {
        exec(
            db,
            "UPDATE runs SET text = ?2 WHERE id = ?1",
            [run.into(), text.into()],
        )
        .await?;
    }
    Ok(())
}
