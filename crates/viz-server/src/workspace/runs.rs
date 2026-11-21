use std::num::NonZeroU64;

use toasty::Executor;

use super::{
    rpc::Patch,
    sql::{Row, exec, row, rows},
    store,
};
use crate::wire::{
    NewEntry, Origin, PromptAttachment, RpcError, Run, RunEnd, RunId, RunStatus, ThreadId,
};

pub(super) const INTERRUPTED: &str =
    "server was interrupted during the ACP turn; the prompt was not resent";

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(super) enum Place {
    End,
    Front,
}

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
    db: &mut dyn Executor,
    thread: u64,
    new: NewRun<'_>,
) -> Result<u64, RpcError> {
    let origin = new.origin.map(store::json).transpose()?;
    let attachments = (!new.attachments.is_empty())
        .then(|| store::json(new.attachments))
        .transpose()?;
    let run = row(db, "INSERT INTO runs (thread, status, position, text, origin, attachments, request_id, prompt_entry)
        VALUES (?1, 'queued', (SELECT CASE WHEN ?2 THEN coalesce(min(position) - 1, 0)
            ELSE coalesce(max(position) + 1, 0) END FROM runs WHERE thread = ?1 AND status = 'queued'),
            ?3, ?4, ?5, ?6, ?7) RETURNING id", [
        thread.into(), (new.place == Place::Front).into(), new.text.into(), origin.into(), attachments.into(),
        new.request_id.into(), new.existing_entry.into(),
    ]).await?.expect("run INSERT RETURNING yielded one row").u64(0)?;
    let title = super::titles::from_prompt(new.text);
    exec(db, "UPDATE threads SET updated_at = ?2, title = CASE WHEN title = '' THEN ?3 ELSE title END WHERE id = ?1",
        [thread.into(), store::now().into(), title.into()]).await?;
    Ok(run)
}

pub(super) async fn by_request(
    db: &mut dyn Executor,
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
    db: &mut dyn Executor,
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
    db: &mut dyn Executor,
    thread: u64,
    run: u64,
) -> Result<Option<Run>, RpcError> {
    row(
        db,
        "SELECT id, status, answer, error FROM runs WHERE thread = ?1 AND id = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(super) async fn latest(db: &mut dyn Executor, thread: u64) -> Result<Option<Run>, RpcError> {
    store::require(db, thread).await?;
    row(
        db,
        "SELECT id, status, answer, error FROM runs WHERE thread = ?1 ORDER BY id DESC LIMIT 1",
        [thread.into()],
    )
    .await?
    .map(|row| decode_run(thread, row))
    .transpose()
}

pub(crate) async fn active_run_from(
    db: &mut dyn Executor,
    thread: u64,
) -> Result<Option<u64>, RpcError> {
    store::require(db, thread).await?;
    active(db, thread).await
}

pub(super) async fn active(db: &mut dyn Executor, thread: u64) -> Result<Option<u64>, RpcError> {
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
        status: decode_status(&row.str(1)?)?,
        answer: row.opt_str(2)?,
        error: row.opt_str(3)?,
    })
}

pub(super) async fn status(
    db: &mut dyn Executor,
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

fn decode_status(value: &str) -> Result<RunStatus, RpcError> {
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

pub(super) const fn unsettled(status: RunStatus) -> bool {
    matches!(status, RunStatus::Queued | RunStatus::Running)
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
pub(super) async fn claim(db: &mut dyn Executor, thread: u64) -> Result<Option<Claimed>, RpcError> {
    let Some(mut row) = row(db, "UPDATE runs SET status = 'running' WHERE thread = ?1 AND status = 'queued'
        AND id = (SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id LIMIT 1)
        RETURNING id, text, origin, attachments, prompt_entry", [thread.into()]).await? else { return Ok(None); };
    let id = row.u64(0)?;
    let text = row.str(1)?;
    let origin = row
        .opt_str(2)?
        .as_deref()
        .map(store::from_json)
        .transpose()?;
    let attachments = row
        .opt_str(3)?
        .as_deref()
        .map(store::from_json)
        .transpose()?
        .unwrap_or_default();
    let (entry, text, origin, attachments) = if let Some(entry) = row.opt_u64(4)? {
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

pub(super) struct Settlement {
    pub(super) status: RunStatus,
    pub(super) end: RunEnd,
}

impl Settlement {
    pub(super) fn failed(error: String) -> Self {
        Self {
            status: RunStatus::Failed,
            end: RunEnd {
                stop_reason: Some("error".into()),
                error: Some(error),
                answer: None,
            },
        }
    }

    pub(super) fn stopped(status: RunStatus, reason: &str, error: &str) -> Self {
        Self {
            status,
            end: RunEnd {
                stop_reason: Some(reason.into()),
                error: Some(error.into()),
                answer: None,
            },
        }
    }
}

pub(super) async fn settle(
    db: &mut dyn Executor,
    thread: u64,
    run: u64,
    settlement: Settlement,
    unread: bool,
) -> Result<(), RpcError> {
    let status = match settlement.status {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
        RunStatus::Queued | RunStatus::Running => unreachable!("settlement is terminal"),
    };
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
    Ok(())
}

pub(super) async fn recover(db: &mut dyn Executor) -> Result<(), RpcError> {
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
            Settlement::stopped(RunStatus::Interrupted, "interrupted", INTERRUPTED),
            true,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn remove_queued(
    db: &mut dyn Executor,
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
    db: &mut dyn Executor,
    thread: u64,
    run: Option<u64>,
) -> Result<(), RpcError> {
    exec(db, "UPDATE runs SET status = 'cancelled', error = 'cancelled before dispatch' WHERE thread = ?1 AND status = 'queued' AND (?2 IS NULL OR id = ?2)",
        [thread.into(), run.into()]).await?;
    Ok(())
}

pub(super) async fn update_queue(
    db: &mut dyn Executor,
    thread: u64,
    run: u64,
    text: Option<&str>,
    before: Patch<NonZeroU64>,
) -> Result<(), RpcError> {
    if status(db, thread, run).await? != Some(RunStatus::Queued) {
        return Err(RpcError::invalid("run is not queued"));
    }
    if match before {
        Patch::Set(target) => target.get() != run,
        Patch::Clear => true,
        Patch::Keep => false,
    } {
        let mut queue = rows(
            db,
            "SELECT id FROM runs WHERE thread = ?1 AND status = 'queued' ORDER BY position, id",
            [thread.into()],
        )
        .await?
        .map(|row| row.u64(0))
        .collect::<Result<Vec<_>, _>>()?;
        queue.retain(|&id| id != run);
        let index = if let Patch::Set(target) = before {
            queue
                .iter()
                .position(|&id| id == target.get())
                .ok_or_else(|| RpcError::invalid("target run is not queued"))?
        } else {
            queue.len()
        };
        queue.insert(index, run);
        // json_each supplies ordinal keys; only rows whose queue position
        // changes need updating.
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
