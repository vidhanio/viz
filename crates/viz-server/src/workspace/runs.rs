//! Durable run persistence and the continuation outbox.

use std::collections::HashSet;

use viz_core::{
    lifecycle,
    runs::{self as decisions, Claim, Move, Place, Settlement},
    threads::Patch,
    titles,
};

use super::{
    sql::{Conn, Row, exec, row, rows},
    store,
};
use crate::{
    error::Error,
    wire::{
        EntryId, NewEntry, Origin, PromptAttachment, PromptContext, Run, RunId, RunKind, RunStatus,
        ThreadId,
    },
};

#[derive(Clone, Copy)]
pub(super) struct NewRun<'a> {
    pub(super) text: &'a str,
    pub(super) origin: Option<&'a Origin>,
    pub(super) attachments: &'a [PromptAttachment],
    pub(super) context: &'a [PromptContext],
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
            context: &[],
            request_id: None,
            existing_entry: None,
            place: Place::End,
        }
    }
}

pub(super) fn enqueue(db: &Conn, thread: ThreadId, new: NewRun<'_>) -> Result<RunId, Error> {
    let origin = new.origin.map(store::json).transpose()?;
    let attachments = (!new.attachments.is_empty())
        .then(|| store::json(new.attachments))
        .transpose()?;
    let context = (!new.context.is_empty())
        .then(|| store::json(new.context))
        .transpose()?;
    let position = queue_position(db, thread, new.place)?;
    let run = row(
        db,
        "INSERT INTO runs (thread, status, position, text, origin, attachments, request_id, prompt_entry, context)
        VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
        [
            thread.into(),
            position.into(),
            new.text.into(),
            origin.into(),
            attachments.into(),
            new.request_id.into(),
            new.existing_entry.into(),
            context.into(),
        ],
    )?
    .expect("run INSERT RETURNING yielded one row")
    .id(0)?;
    let mut thread_row = row(
        db,
        "SELECT title FROM threads WHERE id = ?1",
        [thread.into()],
    )?
    .ok_or_else(|| Error::NoThread(thread))?;
    let title = titles::enqueue(thread_row.str(0)?, new.text);
    exec(
        db,
        "UPDATE threads SET title = ?2 WHERE id = ?1",
        [thread.into(), title.into()],
    )?;
    if new.existing_entry.is_some() {
        store::touch(db, thread, lifecycle::on_agent_activity)?;
    } else {
        store::touch(db, thread, lifecycle::on_input)?;
        exec(
            db,
            "UPDATE threads SET last_input_at = updated_at WHERE id = ?1",
            [thread.into()],
        )?;
    }
    Ok(run)
}

pub(super) fn by_request(
    db: &Conn,
    thread: ThreadId,
    request_id: &str,
) -> Result<Option<RunId>, Error> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND request_id = ?2",
        [thread.into(), request_id.into()],
    )?
    .map(|row| row.id(0))
    .transpose()
}

pub(super) fn origin(db: &Conn, thread: ThreadId, run: RunId) -> Result<Option<Origin>, Error> {
    let Some(mut row) = row(
        db,
        "SELECT origin FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )?
    else {
        return Ok(None);
    };
    row.opt_str(0)?.as_deref().map(store::from_json).transpose()
}

pub(crate) fn run_from(db: &Conn, thread: ThreadId, run: RunId) -> Result<Option<Run>, Error> {
    row(
        db,
        "SELECT id, status, answer, error, kind, usage FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) fn by_ids(
    db: &Conn,
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
    )?
    .map(|record| decode_run(thread, record))
    .collect()
}

pub(super) fn latest(db: &Conn, thread: ThreadId) -> Result<Option<Run>, Error> {
    store::require(db, thread)?;
    row(
        db,
        "SELECT id, status, answer, error, kind, usage FROM runs WHERE thread = ?1 ORDER BY id DESC LIMIT 1",
        [thread.into()],
    )?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) fn active_run_from(db: &Conn, thread: ThreadId) -> Result<Option<RunId>, Error> {
    store::require(db, thread)?;
    active(db, thread)
}

pub(super) fn active(db: &Conn, thread: ThreadId) -> Result<Option<RunId>, Error> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'running'",
        [thread.into()],
    )?
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

pub(super) fn status(db: &Conn, thread: ThreadId, run: RunId) -> Result<Option<RunStatus>, Error> {
    row(
        db,
        "SELECT status FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )?
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
    pub(super) context: Vec<PromptContext>,
}

/// What a worker found to do.
#[derive(Default)]
pub(super) struct Found {
    pub(super) claimed: Option<Claimed>,
    /// A run nobody was on was settled.
    pub(super) recovered: bool,
}

/// Commit the claim and corresponding prompt together before starting any
/// external operation.
pub(super) fn claim(db: &Conn, thread: ThreadId) -> Result<Found, Error> {
    let active = active(db, thread)?;
    let next = row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id LIMIT 1",
        [thread.into()],
    )?
    .map(|row| row.id(0))
    .transpose()?;
    let (id, recovered) = match decisions::claim(active, next) {
        Claim::Nothing => (None, false),
        Claim::Start(id) => (Some(id), false),
        Claim::Orphan(run) => {
            settle(db, thread, run, Settlement::orphaned(), false)?;
            (next, true)
        }
    };
    let claimed = match id {
        Some(id) => Some(start(db, thread, id)?),
        None => None,
    };
    Ok(Found { claimed, recovered })
}

fn start(db: &Conn, thread: ThreadId, id: RunId) -> Result<Claimed, Error> {
    let mut row = row(
        db,
        "UPDATE runs SET status = 'running' WHERE id = ?1
        RETURNING text, origin, attachments, prompt_entry, context",
        [id.into()],
    )?
    .expect("claimed run exists under the write transaction");
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
    let context: Vec<PromptContext> = row
        .opt_str(4)?
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
            context: context.clone(),
            truncated: false,
        };
        let entry = store::append(db, thread, &data)?;
        exec(
            db,
            "UPDATE runs SET prompt_entry = ?2 WHERE id = ?1",
            [id.into(), entry.into()],
        )?;
        let NewEntry::Prompt {
            text,
            origin,
            attachments,
            ..
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
        context,
    })
}

/// Starts an unprompted run: the agent sent transcript content with no turn
/// open. It is created `running`, with a `continued` entry in place of a
/// prompt, and returns the run and that entry. Its caller holds the worker's
/// slot, so the `runs_active` index cannot conflict.
pub(super) fn begin_unprompted(db: &Conn, thread: ThreadId) -> Result<(RunId, EntryId), Error> {
    let entry = store::append(db, thread, &NewEntry::Continued)?;
    let run = row(
        db,
        "INSERT INTO runs (thread, kind, status, position, text, prompt_entry)
        VALUES (?1, 'unprompted', 'running', 0, '', ?2) RETURNING id",
        [thread.into(), entry.into()],
    )?
    .expect("run INSERT RETURNING yielded one row")
    .id(0)?;
    store::touch(db, thread, lifecycle::on_agent_activity)?;
    Ok((run, entry))
}

/// Whether inputs are waiting on the thread.
pub(super) fn queued(db: &Conn, thread: ThreadId) -> Result<bool, Error> {
    Ok(row(
        db,
        "SELECT 1 FROM runs WHERE thread = ?1 AND status = 'queued' LIMIT 1",
        [thread.into()],
    )?
    .is_some())
}

pub(super) fn settle(
    db: &Conn,
    thread: ThreadId,
    run: RunId,
    settlement: Settlement,
    watched: bool,
) -> Result<(), Error> {
    let ended = settlement.status.into();
    let status = status_name(ended);
    let data = NewEntry::Run(settlement.end);
    let entry = store::append(db, thread, &data)?;
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
    )?;
    store::withdraw_requests(db, Some(run), viz_core::requests::on_settle(ended))?;
    store::touch(db, thread, |current| {
        lifecycle::on_run_end(current, ended, watched)
    })?;
    exec(
        db,
        "UPDATE threads SET setup_failed = 0 WHERE id = ?1",
        [thread.into()],
    )?;
    super::follow_ups::record(db, thread, run)
}

pub(super) fn recover(db: &Conn) -> Result<(), Error> {
    store::withdraw_requests(db, None, viz_protocol::Withdrawal::Restarted)?;
    for row in rows(
        db,
        "SELECT id, thread FROM runs WHERE status = 'running'",
        [],
    )? {
        settle(db, row.id(1)?, row.id(0)?, Settlement::interrupted(), false)?;
    }
    Ok(())
}

fn queue_position(db: &Conn, thread: ThreadId, place: Place) -> Result<i64, Error> {
    let bounds = row(
        db,
        "SELECT min(position), max(position) FROM runs
        WHERE thread = ?1 AND status = 'queued'",
        [thread.into()],
    )?
    .expect("aggregate query returns a row");
    let bounds = bounds.opt_i64(0)?.zip(bounds.opt_i64(1)?);
    place
        .position(bounds)
        .ok_or_else(|| Error::corrupt("run queue position overflow"))
}

/// The queued runs in dispatch order.
fn queue(db: &Conn, thread: ThreadId) -> Result<Vec<RunId>, Error> {
    rows(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id",
        [thread.into()],
    )?
    .map(|row| row.id(0))
    .collect()
}

fn save_order(db: &Conn, queue: &[RunId]) -> Result<(), Error> {
    exec(
        db,
        "UPDATE runs AS r SET position = j.key FROM json_each(?1) AS j
         WHERE r.id = j.value AND r.position <> j.key",
        [store::json(queue)?.into()],
    )?;
    Ok(())
}

fn cancel_queued(db: &Conn, runs: &[RunId], reason: &str) -> Result<(), Error> {
    exec(
        db,
        "UPDATE runs SET status = 'cancelled', error = ?2
         WHERE id IN (SELECT value FROM json_each(?1))",
        [store::json(runs)?.into(), reason.into()],
    )?;
    Ok(())
}

/// Move a queued run to the front, keeping its identity and payload.
pub(super) fn promote(db: &Conn, thread: ThreadId, run: RunId) -> Result<(), Error> {
    let order = decisions::promote(&queue(db, thread)?, run)?;
    save_order(db, &order)
}

pub(super) fn remove(db: &Conn, thread: ThreadId, run: RunId) -> Result<(), Error> {
    let run = decisions::remove(&queue(db, thread)?, run)?;
    cancel_queued(db, &[run], "removed from queue")
}

/// Cancel the queued runs a stop covers, and return the running one for the
/// worker to stop.
pub(super) fn cancel(
    db: &Conn,
    thread: ThreadId,
    wanted: Option<RunId>,
) -> Result<Option<RunId>, Error> {
    let cancellation = decisions::cancel(queue(db, thread)?, active(db, thread)?, wanted);
    cancel_queued(db, &cancellation.queued, "cancelled before dispatch")?;
    Ok(cancellation.active)
}

pub(super) fn idle(db: &Conn, thread: ThreadId) -> Result<(), Error> {
    let queued = queued(db, thread)?;
    Ok(decisions::idle(queued, active(db, thread)?)?)
}

pub(super) fn update_queue(
    db: &Conn,
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
    if let Some(order) = decisions::reorder(queue(db, thread)?, run, placement)? {
        save_order(db, &order)?;
    }
    if let Some(text) = text {
        exec(
            db,
            "UPDATE runs SET text = ?2 WHERE id = ?1",
            [run.into(), text.into()],
        )?;
    }
    Ok(())
}
