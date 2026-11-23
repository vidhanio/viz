//! SQLite persistence for tasks and stopped run families.

use std::collections::HashSet;

use viz_core::tasks::{self as decisions, Delivery, Family, Phase, WakeEdge, WakeMode};
use viz_protocol::{TaskId, peer::Received};

use crate::{
    error::Error,
    wire::{RunId, TaskStatus, ThreadId},
    workspace::{
        run_from,
        runs::by_ids,
        sql::{self, Connection, Row},
    },
};

/// A task as stored: its identity, and the state decisions are made on.
pub(super) struct Stored {
    pub(super) id: TaskId,
    pub(super) parent: ThreadId,
    pub(super) parent_run: Option<RunId>,
    pub(super) environment: String,
    pub(super) title: String,
    pub(super) state: decisions::Task,
}

/// A task to record.
pub(super) struct New<'a> {
    pub(super) id: &'a TaskId,
    pub(super) parent: ThreadId,
    pub(super) parent_run: Option<RunId>,
    pub(super) environment: &'a str,
    pub(super) title: &'a str,
    pub(super) payload: &'a str,
    pub(super) request_id: Option<&'a str>,
}

macro_rules! select_task {
    ($condition:literal) => {
        concat!(
            "SELECT id, parent, parent_run, environment, title, phase, thread, run, status, ",
            "delivery, wake_run, result, error FROM agent_tasks WHERE ",
            $condition,
        )
    };
}

/// Read one task, or only if `parent` owns it.
pub(super) async fn read(
    db: &mut Connection,
    id: &TaskId,
    parent: Option<ThreadId>,
) -> Result<Stored, Error> {
    sql::row(
        db,
        select_task!("id = ?1 AND (?2 IS NULL OR parent = ?2)"),
        [id.into(), parent.into()],
    )
    .await?
    .map(decode)
    .transpose()?
    .ok_or_else(|| Error::NoTask(id.clone()))
}

/// The tasks of `parent`, or all, newest first.
pub(super) async fn list(
    db: &mut Connection,
    parent: Option<ThreadId>,
) -> Result<Vec<Stored>, Error> {
    sql::rows(
        db,
        select_task!("(?1 IS NULL OR parent = ?1) ORDER BY rowid DESC"),
        [parent.into()],
    )
    .await?
    .map(decode)
    .collect()
}

fn decode(mut row: Row) -> Result<Stored, Error> {
    let receipt = match (row.opt_id(6)?, row.opt_id(7)?) {
        (Some(thread), Some(run)) => Some(Received { thread, run }),
        (None, None) => None,
        _ => return Err(Error::corrupt("half a task receipt")),
    };
    let phase = match (row.str(5)?.as_str(), receipt) {
        ("pending", None) => Phase::Pending,
        ("offered", None) => Phase::Offered,
        ("retracting", None) => Phase::Retracting,
        ("accepted", Some(receipt)) => Phase::Accepted(receipt),
        ("cancelling", Some(receipt)) => Phase::Cancelling(receipt),
        ("settled", receipt) => Phase::Settled(receipt),
        (phase, _) => return Err(Error::corrupt(format_args!("task phase {phase}"))),
    };
    let delivery = match row.str(9)?.as_str() {
        "prompt" => Delivery::Owed(WakeMode::Prompt),
        "entry_only" => Delivery::Owed(WakeMode::EntryOnly),
        "delivered" => Delivery::Delivered(row.opt_id(10)?),
        delivery => return Err(Error::corrupt(format_args!("task delivery {delivery}"))),
    };
    let status = match row.str(8)?.as_str() {
        "queued" => TaskStatus::Queued,
        "running" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "cancelled" => TaskStatus::Cancelled,
        status => return Err(Error::corrupt(format_args!("task status {status}"))),
    };
    Ok(Stored {
        id: TaskId(row.str(0)?),
        parent: row.id(1)?,
        parent_run: row.opt_id(2)?,
        environment: row.str(3)?,
        title: row.str(4)?,
        state: decisions::Task {
            phase,
            status,
            delivery,
            result: row.opt_str(11)?,
            error: row.opt_str(12)?,
        },
    })
}

pub(super) async fn insert(db: &mut Connection, new: New<'_>) -> Result<(), Error> {
    sql::exec(
        db,
        "INSERT INTO agent_tasks (id, parent, parent_run, environment, status, title, payload, request_id, phase, delivery)
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, 'pending', 'prompt')",
        [
            new.id.into(),
            new.parent.into(),
            new.parent_run.into(),
            new.environment.into(),
            new.title.into(),
            new.payload.into(),
            new.request_id.into(),
        ],
    )
    .await?;
    Ok(())
}

/// Store a task's new state, dropping the payload once its offer is over.
pub(super) async fn save(
    db: &mut Connection,
    id: &TaskId,
    task: &decisions::Task,
) -> Result<(), Error> {
    let phase = match task.phase {
        Phase::Pending => "pending",
        Phase::Offered => "offered",
        Phase::Retracting => "retracting",
        Phase::Accepted(_) => "accepted",
        Phase::Cancelling(_) => "cancelling",
        Phase::Settled(_) => "settled",
    };
    let status = match task.status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    };
    let (delivery, wake) = match task.delivery {
        Delivery::Owed(WakeMode::Prompt) => ("prompt", None),
        Delivery::Owed(WakeMode::EntryOnly) => ("entry_only", None),
        Delivery::Delivered(wake) => ("delivered", wake),
    };
    let receipt = task.phase.receipt();
    sql::exec(
        db,
        "UPDATE agent_tasks SET phase = ?2, thread = ?3, run = ?4, status = ?5, delivery = ?6,
            wake_run = ?7, result = ?8, error = ?9, payload = CASE WHEN ?10 THEN payload END
         WHERE id = ?1",
        [
            id.into(),
            phase.into(),
            receipt.map(|receipt| receipt.thread).into(),
            receipt.map(|receipt| receipt.run).into(),
            status.into(),
            delivery.into(),
            wake.into(),
            task.result.as_deref().into(),
            task.error.as_deref().into(),
            task.phase.offerable().into(),
        ],
    )
    .await?;
    Ok(())
}

/// The offer to send for a task, while it may still be sent.
pub(super) async fn payload(db: &mut Connection, id: &TaskId) -> Result<Option<String>, Error> {
    sql::row(
        db,
        "SELECT payload FROM agent_tasks WHERE id = ?1",
        [id.into()],
    )
    .await?
    .map_or(Ok(None), |mut row| row.opt_str(0))
}

pub(super) async fn prior_request(
    db: &mut Connection,
    parent: ThreadId,
    request: &str,
) -> Result<Option<TaskId>, Error> {
    sql::row(
        db,
        "SELECT id FROM agent_tasks WHERE parent = ?1 AND request_id = ?2",
        [parent.into(), request.into()],
    )
    .await?
    .map(|mut row| row.str(0).map(TaskId))
    .transpose()
}

pub(super) async fn stop_exists(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<bool, Error> {
    Ok(sql::row(
        db,
        "SELECT handled FROM stopped_task_runs WHERE thread = ?1 AND run = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .is_some())
}

/// Keep a stopped family's undelivered results from waking anyone, and cancel
/// what is still running.
pub(super) async fn suppress(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<(), Error> {
    for task in family(db, thread, run).await?.tasks {
        if let Some(next) = decisions::decide(&task.state, decisions::Command::Suppress) {
            save(db, &task.id, &next).await?;
        }
    }
    Ok(())
}

pub(super) async fn snapshot(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<Family, Error> {
    let root = run_from(db, thread, run).await?;
    let family = family(db, thread, run).await?;
    let pending_delivery = family
        .tasks
        .iter()
        .any(|task| matches!(task.state.delivery, Delivery::Owed(_)));
    let wakes = by_ids(db, thread, &family.wakes).await?;
    Ok(Family::from_runs(root, wakes, pending_delivery))
}

struct FamilyRows {
    tasks: Vec<Stored>,
    wakes: HashSet<RunId>,
}

/// Original result wakes join the family after delivery is recorded. Follow-up
/// wakes join on their receipt, which is persisted with the new run.
async fn family(db: &mut Connection, thread: ThreadId, run: RunId) -> Result<FamilyRows, Error> {
    let tasks = sql::rows(
        db,
        select_task!("parent = ?1 AND parent_run IS NOT NULL"),
        [thread.into()],
    )
    .await?
    .map(decode)
    .collect::<Result<Vec<_>, Error>>()?;
    let mut edges = sql::rows(
        db,
        "SELECT a.parent_run, w.run FROM task_wakes w
         JOIN agent_tasks a ON a.id = w.task AND a.parent = w.thread
         WHERE w.thread = ?1 AND w.sequence > 0 AND w.run IS NOT NULL AND a.parent_run IS NOT NULL",
        [thread.into()],
    )
    .await?
    .map(|row| {
        Ok(WakeEdge {
            parent: row.id(0)?,
            wake: row.id(1)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    edges.extend(tasks.iter().filter_map(|task| {
        let Delivery::Delivered(wake) = task.state.delivery else {
            return None;
        };
        Some(WakeEdge {
            parent: task.parent_run?,
            wake: wake?,
        })
    }));
    let mut wakes = decisions::descendants(run, edges);
    let tasks = tasks
        .into_iter()
        .filter(|task| {
            task.parent_run
                .is_some_and(|parent| wakes.contains(&parent))
        })
        .collect();
    wakes.remove(&run);
    Ok(FamilyRows { tasks, wakes })
}
