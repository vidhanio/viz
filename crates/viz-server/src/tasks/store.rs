//! SQLite persistence for tasks and stopped run families.

use std::collections::HashSet;

use viz_core::tasks::{self as decisions, Delivery, Family, Orphan, Phase, WakeEdge, WakeMode};
use viz_protocol::{TaskId, peer::Received};

use crate::{
    error::Error,
    wire::{RunId, TaskStatus, ThreadId},
    workspace::{
        run_from,
        runs::by_ids,
        sql::{self, Conn, Row},
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
#[derive(Clone, Copy)]
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
            "delivery, wake_run, result, error, dismissed FROM agent_tasks WHERE ",
            $condition,
        )
    };
}

/// Read one task, or only if `parent` owns it.
pub(super) fn read(db: &Conn, id: &TaskId, parent: Option<ThreadId>) -> Result<Stored, Error> {
    sql::row(
        db,
        select_task!("id = ?1 AND (?2 IS NULL OR parent = ?2)"),
        [id.into(), parent.into()],
    )?
    .map(decode)
    .transpose()?
    .ok_or_else(|| Error::NoTask(id.clone()))
}

/// The tasks listed under `parent`, or under any thread, newest first.
pub(super) fn list(db: &Conn, parent: Option<ThreadId>) -> Result<Vec<Stored>, Error> {
    sql::rows(
        db,
        select_task!("dismissed = 0 AND (?1 IS NULL OR parent = ?1) ORDER BY rowid DESC"),
        [parent.into()],
    )?
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
    let delivery = decode_delivery(&mut row, 9)?;
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
            dismissed: row.flag(13)?,
        },
    })
}

/// Read the delivery column and the wake column after it.
fn decode_delivery(row: &mut Row, column: usize) -> Result<Delivery, Error> {
    match row.str(column)?.as_str() {
        "prompt" => Ok(Delivery::Owed(WakeMode::Prompt)),
        "entry_only" => Ok(Delivery::Owed(WakeMode::EntryOnly)),
        "delivered" => Ok(Delivery::Delivered(row.opt_id(column + 1)?)),
        delivery => Err(Error::corrupt(format_args!("task delivery {delivery}"))),
    }
}

/// Remove a task nothing can refer to.
pub(super) fn remove(db: &Conn, id: &TaskId) -> Result<(), Error> {
    sql::exec(db, "DELETE FROM agent_tasks WHERE id = ?1", [id.into()])?;
    Ok(())
}

/// Release what a deleted thread delegated, and its stop requests. Tasks still
/// under way are stopped first (see [`decisions::orphan`]).
pub(crate) fn release(db: &Conn, parent: ThreadId) -> Result<(), Error> {
    let tasks = sql::rows(db, select_task!("parent = ?1"), [parent.into()])?
        .map(decode)
        .collect::<Result<Vec<_>, Error>>()?;
    for task in tasks {
        match decisions::orphan(&task.state) {
            Orphan::Remove => remove(db, &task.id)?,
            Orphan::Stop(next) => save(db, &task.id, &next)?,
        }
    }
    sql::exec(
        db,
        "DELETE FROM stopped_task_runs WHERE thread = ?1",
        [parent.into()],
    )?;
    Ok(())
}

pub(super) fn insert(db: &Conn, new: New<'_>) -> Result<(), Error> {
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
    )?;
    Ok(())
}

/// Store a task's new state, dropping the payload once its offer is over.
pub(super) fn save(db: &Conn, id: &TaskId, task: &decisions::Task) -> Result<(), Error> {
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
            wake_run = ?7, result = ?8, error = ?9, payload = CASE WHEN ?10 THEN payload END,
            dismissed = ?11
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
            task.dismissed.into(),
        ],
    )?;
    Ok(())
}

/// The offer to send for a task, while it may still be sent.
pub(super) fn payload(db: &Conn, id: &TaskId) -> Result<Option<String>, Error> {
    sql::row(
        db,
        "SELECT payload FROM agent_tasks WHERE id = ?1",
        [id.into()],
    )?
    .map_or(Ok(None), |mut row| row.opt_str(0))
}

pub(super) fn prior_request(
    db: &Conn,
    parent: ThreadId,
    request: &str,
) -> Result<Option<TaskId>, Error> {
    sql::row(
        db,
        "SELECT id FROM agent_tasks WHERE parent = ?1 AND request_id = ?2",
        [parent.into(), request.into()],
    )?
    .map(|mut row| row.str(0).map(TaskId))
    .transpose()
}

pub(super) fn stop_exists(db: &Conn, thread: ThreadId, run: RunId) -> Result<bool, Error> {
    Ok(sql::row(
        db,
        "SELECT handled FROM stopped_task_runs WHERE thread = ?1 AND run = ?2",
        [thread.into(), run.into()],
    )?
    .is_some())
}

/// Keep a stopped family's undelivered results from waking anyone, and cancel
/// what is still running.
pub(super) fn suppress(db: &Conn, thread: ThreadId, run: RunId) -> Result<(), Error> {
    for member in family(db, thread, run)?.tasks {
        if matches!(member.delivery, Delivery::Delivered(_)) {
            continue;
        }
        let task = read(db, &member.id, None)?;
        if let Some(next) = decisions::decide(&task.state, decisions::Command::Suppress) {
            save(db, &task.id, &next)?;
        }
    }
    Ok(())
}

pub(super) fn snapshot(db: &Conn, thread: ThreadId, run: RunId) -> Result<Family, Error> {
    let root = run_from(db, thread, run)?;
    let family = family(db, thread, run)?;
    let pending_delivery = family
        .tasks
        .iter()
        .any(|task| matches!(task.delivery, Delivery::Owed(_)));
    let wakes = by_ids(db, thread, &family.wakes)?;
    Ok(Family::from_runs(root, wakes, pending_delivery))
}

struct FamilyRows {
    tasks: Vec<Member>,
    wakes: HashSet<RunId>,
}

/// What a family needs to know of a task, which excludes its result.
struct Member {
    id: TaskId,
    parent: RunId,
    delivery: Delivery,
}

/// Original result wakes join the family after delivery is recorded. Follow-up
/// wakes join on their receipt, which is persisted with the new run.
fn family(db: &Conn, thread: ThreadId, run: RunId) -> Result<FamilyRows, Error> {
    let tasks = sql::rows(
        db,
        "SELECT id, parent_run, delivery, wake_run FROM agent_tasks
         WHERE parent = ?1 AND parent_run IS NOT NULL",
        [thread.into()],
    )?
    .map(|mut row| {
        Ok(Member {
            id: TaskId(row.str(0)?),
            parent: row.id(1)?,
            delivery: decode_delivery(&mut row, 2)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    let mut edges = sql::rows(
        db,
        "SELECT a.parent_run, w.run FROM task_wakes w
         JOIN agent_tasks a ON a.id = w.task AND a.parent = w.thread
         WHERE w.thread = ?1 AND w.sequence > 0 AND w.run IS NOT NULL AND a.parent_run IS NOT NULL",
        [thread.into()],
    )?
    .map(|row| {
        Ok(WakeEdge {
            parent: row.id(0)?,
            wake: row.id(1)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    edges.extend(tasks.iter().filter_map(|task| {
        let Delivery::Delivered(wake) = task.delivery else {
            return None;
        };
        Some(WakeEdge {
            parent: task.parent,
            wake: wake?,
        })
    }));
    let mut wakes = decisions::descendants(run, edges);
    let tasks = tasks
        .into_iter()
        .filter(|task| wakes.contains(&task.parent))
        .collect();
    wakes.remove(&run);
    Ok(FamilyRows { tasks, wakes })
}
