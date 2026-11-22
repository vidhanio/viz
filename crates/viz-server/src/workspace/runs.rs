//! Durable run persistence and the continuation outbox.

use std::num::NonZeroU64;

use viz_core::{
    runs::{self as decisions, Move, Place, QueueRejection, Settlement},
    threads::Patch,
    titles,
};

use super::{
    sql::{Row, exec, row, rows},
    store,
};
use crate::{
    wire::{
        NewEntry, Origin, PromptAttachment, RpcError, Run, RunId, RunKind, RunStatus, ThreadId,
    },
    workspace::sql::Connection,
};

pub(super) struct NewRun<'a> {
    pub(super) text: &'a str,
    pub(super) origin: Option<&'a Origin>,
    pub(super) attachments: &'a [PromptAttachment],
    pub(super) request_id: Option<&'a str>,
    pub(super) existing_entry: Option<u64>,
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
    thread: u64,
    new: NewRun<'_>,
) -> Result<u64, RpcError> {
    let origin = new.origin.map(store::json).transpose()?;
    let attachments = (!new.attachments.is_empty())
        .then(|| store::json(new.attachments))
        .transpose()?;
    let position = queue_position(db, thread, new.place, None).await?;
    let run = row(db, "INSERT INTO runs (thread, status, position, text, origin, attachments, request_id, prompt_entry)
        VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id", [
        thread.into(), position.into(), new.text.into(), origin.into(), attachments.into(),
        new.request_id.into(), new.existing_entry.into(),
    ]).await?.expect("run INSERT RETURNING yielded one row").u64(0)?;
    let mut thread_row = row(
        db,
        "SELECT title FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    .ok_or_else(|| store::missing(thread))?;
    let title = titles::enqueue(thread_row.str(0)?, new.text);
    exec(
        db,
        "UPDATE threads SET updated_at = ?2, title = ?3 WHERE id = ?1",
        [thread.into(), store::now().into(), title.into()],
    )
    .await?;
    Ok(run)
}

pub(super) async fn by_request(
    db: &mut Connection,
    thread: u64,
    request_id: &str,
) -> Result<Option<u64>, RpcError> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND request_id = ?2",
        [thread.into(), request_id.into()],
    )
    .await?
    .map(|row| row.u64(0))
    .transpose()
}

pub(super) async fn origin(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<Option<Origin>, RpcError> {
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
    thread: u64,
    run: u64,
) -> Result<Option<Run>, RpcError> {
    row(
        db,
        "SELECT id, status, answer, error, kind FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(super) async fn latest(db: &mut Connection, thread: u64) -> Result<Option<Run>, RpcError> {
    store::require(db, thread).await?;
    row(
        db,
        "SELECT id, status, answer, error, kind FROM runs WHERE thread = ?1 ORDER BY id DESC LIMIT 1",
        [thread.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) async fn active_run_from(
    db: &mut Connection,
    thread: u64,
) -> Result<Option<u64>, RpcError> {
    store::require(db, thread).await?;
    active(db, thread).await
}

pub(super) async fn active(db: &mut Connection, thread: u64) -> Result<Option<u64>, RpcError> {
    row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'running'",
        [thread.into()],
    )
    .await?
    .map(|row| row.u64(0))
    .transpose()
}

fn decode_run(thread: u64, mut row: Row) -> Result<Run, RpcError> {
    Ok(Run {
        id: RunId(row.u64(0)?),
        thread: ThreadId(thread),
        kind: if row.str(4)? == "unprompted" {
            RunKind::Unprompted
        } else {
            RunKind::Prompt
        },
        status: decode_status(&row.str(1)?)?,
        answer: row.opt_str(2)?,
        error: row.opt_str(3)?,
    })
}

pub(super) async fn status(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<Option<RunStatus>, RpcError> {
    row(
        db,
        "SELECT status FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .map(|mut row| decode_status(&row.str(0)?))
    .transpose()
}

pub(super) fn decode_status(value: &str) -> Result<RunStatus, RpcError> {
    Ok(match value {
        "queued" => RunStatus::Queued,
        "running" => RunStatus::Running,
        "completed" => RunStatus::Completed,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        "interrupted" => RunStatus::Interrupted,
        _ => {
            return Err(RpcError::internal(format_args!(
                "unknown persisted run status {value}"
            )));
        }
    })
}

pub(super) struct Claimed {
    pub(super) id: u64,
    pub(super) entry: u64,
    pub(super) text: String,
    pub(super) origin: Option<Origin>,
    pub(super) attachments: Vec<PromptAttachment>,
}

/// Commit the claim and corresponding prompt together before starting any
/// external operation.
pub(super) async fn claim(db: &mut Connection, thread: u64) -> Result<Option<Claimed>, RpcError> {
    let active = active(db, thread).await?.map(RunId);
    let next = row(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id LIMIT 1",
        [thread.into()],
    )
    .await?
    .map(|row| row.u64(0).map(RunId))
    .transpose()?;
    let Some(RunId(id)) = decisions::claim(active, next) else {
        return Ok(None);
    };
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
    let (entry, text, origin, attachments) = if let Some(entry) = row.opt_u64(3)? {
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
    Ok(Some(Claimed {
        id,
        entry,
        text,
        origin,
        attachments,
    }))
}

/// Starts an unprompted run: the agent sent transcript content with no turn
/// open. It is created `running`, with a `continued` entry in place of a
/// prompt, and returns the run and that entry. Its caller holds the worker's
/// slot, so the `runs_active` index cannot conflict.
pub(super) async fn begin_unprompted(
    db: &mut Connection,
    thread: u64,
) -> Result<(u64, u64), RpcError> {
    let entry = store::append(db, thread, &NewEntry::Continued).await?;
    let run = row(
        db,
        "INSERT INTO runs (thread, kind, status, position, text, prompt_entry)
        VALUES (?1, 'unprompted', 'running', 0, '', ?2) RETURNING id",
        [thread.into(), entry.into()],
    )
    .await?
    .expect("run INSERT RETURNING yielded one row")
    .u64(0)?;
    exec(
        db,
        "UPDATE threads SET updated_at = ?2 WHERE id = ?1",
        [thread.into(), store::now().into()],
    )
    .await?;
    Ok((run, entry))
}

/// Whether inputs are waiting on the thread.
pub(super) async fn queued(db: &mut Connection, thread: u64) -> Result<bool, RpcError> {
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
    thread: u64,
    run: u64,
    settlement: Settlement,
    unread: bool,
) -> Result<(), RpcError> {
    let status = settlement.status.as_str();
    let data = NewEntry::Run(settlement.end);
    let entry = store::append(db, thread, &data).await?;
    let NewEntry::Run(end) = data else {
        unreachable!()
    };
    exec(
        db,
        "UPDATE runs SET status = ?2, answer = ?3, error = ?4, end_entry = ?5 WHERE id = ?1",
        [
            run.into(),
            status.into(),
            end.answer.into(),
            end.error.into(),
            entry.into(),
        ],
    )
    .await?;
    exec(db, "UPDATE threads SET updated_at = ?2, unread = CASE WHEN ?3 THEN 1 ELSE unread END, setup_failed = 0 WHERE id = ?1",
        [thread.into(), store::now().into(), unread.into()]).await?;
    follow_up(db, thread, run).await
}

/// Queue the answer only when the preceding prompt belongs to a task receipt
/// or a wake in that task's run family. Manual inputs break this association.
/// The outbox row commits with the run, and the parent deduplicates retries
/// using the continuation run ID.
async fn follow_up(db: &mut Connection, thread: u64, run: u64) -> Result<(), RpcError> {
    exec(
        db,
        "WITH RECURSIVE lineage(id) AS (
            SELECT r.id FROM runs u JOIN runs r ON r.id = (
                SELECT id FROM runs WHERE thread = u.thread AND kind = 'prompt'
                    AND end_entry < u.prompt_entry ORDER BY end_entry DESC LIMIT 1
            ) WHERE u.id = ?2 AND u.thread = ?1 AND u.kind = 'unprompted'
                AND u.status = 'completed' AND r.status = 'completed'
                AND trim(coalesce(u.answer, '')) <> ''
            UNION
            SELECT a.parent_run FROM lineage l JOIN task_wakes w ON w.run = l.id AND w.thread = ?1
                JOIN agent_tasks a ON a.id = w.task AND a.parent = w.thread
                WHERE a.parent_run IS NOT NULL
        )
        INSERT OR IGNORE INTO task_follow_ups (environment, task, run, thread, text)
        SELECT tr.environment, tr.task, u.id, u.thread, u.answer
        FROM runs u JOIN task_receipts tr ON tr.thread = u.thread
        WHERE u.id = ?2 AND tr.run IN (SELECT id FROM lineage)
            AND NOT EXISTS (SELECT 1 FROM stopped_task_runs s WHERE s.thread = tr.thread AND s.run = tr.run)",
        [thread.into(), run.into()],
    )
    .await?;
    Ok(())
}

pub(super) async fn recover(db: &mut Connection) -> Result<(), RpcError> {
    for row in rows(
        db,
        "SELECT id, thread FROM runs WHERE status = 'running'",
        [],
    )
    .await?
    {
        settle(
            db,
            row.u64(1)?,
            row.u64(0)?,
            Settlement::interrupted(),
            true,
        )
        .await?;
    }
    Ok(())
}

async fn queue_position(
    db: &mut Connection,
    thread: u64,
    place: Place,
    exclude: Option<u64>,
) -> Result<i64, RpcError> {
    let bounds = row(
        db,
        "SELECT min(position), max(position) FROM runs
        WHERE thread = ?1 AND status = 'queued' AND (?2 IS NULL OR id <> ?2)",
        [thread.into(), exclude.into()],
    )
    .await?
    .expect("aggregate query returns a row");
    let bounds = bounds.opt_i64(0)?.zip(bounds.opt_i64(1)?);
    place
        .position(bounds)
        .ok_or_else(|| RpcError::internal("run queue position overflow"))
}

/// Move the existing run ahead of other queued inputs, preserving its payload.
pub(super) async fn promote_queued(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<(), RpcError> {
    if status(db, thread, run).await? != Some(RunStatus::Queued) {
        return Err(RpcError::invalid(QueueRejection::NotQueued.to_string()));
    }
    let position = queue_position(db, thread, Place::Front, Some(run)).await?;
    exec(
        db,
        "UPDATE runs SET position = ?2 WHERE id = ?1",
        [run.into(), position.into()],
    )
    .await?;
    Ok(())
}

pub(super) async fn remove_queued(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<(), RpcError> {
    let changed = exec(db, "UPDATE runs SET status = 'cancelled', error = 'removed from queue' WHERE thread = ?1 AND id = ?2 AND status = 'queued'",
        [thread.into(), run.into()]).await?;
    if changed == 0 {
        return Err(RpcError::invalid("run is not queued"));
    }
    Ok(())
}

pub(super) async fn cancel_queued(
    db: &mut Connection,
    thread: u64,
    run: Option<u64>,
) -> Result<(), RpcError> {
    exec(db, "UPDATE runs SET status = 'cancelled', error = 'cancelled before dispatch' WHERE thread = ?1 AND status = 'queued' AND (?2 IS NULL OR id = ?2)",
        [thread.into(), run.into()]).await?;
    Ok(())
}

pub(super) async fn update_queue(
    db: &mut Connection,
    thread: u64,
    run: u64,
    text: Option<&str>,
    before: Patch<NonZeroU64>,
) -> Result<(), RpcError> {
    let queue = rows(
        db,
        "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id",
        [thread.into()],
    )
    .await?
    .map(|row| row.u64(0).map(RunId))
    .collect::<Result<Vec<_>, _>>()?;
    let placement = match before {
        Patch::Set(target) => Move::Before(RunId(target.get())),
        Patch::Clear => Move::End,
        Patch::Keep => Move::Keep,
    };
    if let Some(queue) = decisions::reorder(queue, RunId(run), placement)
        .map_err(|error| RpcError::invalid(error.to_string()))?
    {
        exec(db, "UPDATE runs AS r SET position = j.key FROM json_each(?1) AS j WHERE r.id = j.value AND r.position <> j.key",
            [store::json(&queue)?.into()]).await?;
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
