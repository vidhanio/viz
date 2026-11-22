//! SQLite persistence for tasks and stopped run families. Phase decisions
//! consume the committed row under the workspace write lease.

use std::collections::HashSet;

use viz_core::tasks::{self as decisions, Command, Family, Phase, Transition, WakeEdge};
use viz_protocol::TaskId;

use crate::{
    error::Error,
    wire::{RunId, TaskStatus, ThreadId},
    workspace::{
        run_from,
        runs::by_ids,
        sql::{self, Connection},
    },
};

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

pub(super) async fn task_phase(db: &mut Connection, task: &TaskId) -> Result<Option<Phase>, Error> {
    sql::row(
        db,
        "SELECT phase FROM agent_tasks WHERE id = ?1",
        [task.into()],
    )
    .await?
    .map(|mut row| row.str(0)?.parse::<Phase>().map_err(Error::corrupt))
    .transpose()
}

pub(super) async fn finish_task(
    db: &mut Connection,
    task: &TaskId,
    status: TaskStatus,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<(), Error> {
    sql::exec(db,
        "UPDATE agent_tasks SET status = ?2, result = ?3, error = ?4, phase = 'settled', payload = NULL WHERE id = ?1",
        [task.into(), decisions::status_name(status).into(), result.into(), error.into()],
    ).await?;
    Ok(())
}

/// Cancellation may change the phase and independently suppress future prompt
/// delivery.
pub(super) async fn cancel_task(
    db: &mut Connection,
    task: &TaskId,
    phase: Phase,
    delivery: Option<&str>,
) -> Result<(), Error> {
    let (phase, status) = match decisions::decide(phase, Command::Cancel) {
        Transition::Keep if delivery.is_none() => return Ok(()),
        Transition::Keep => (phase, None),
        Transition::Move(next) => (next, None),
        Transition::Settle(status) => (Phase::Settled, Some(decisions::status_name(status))),
    };
    sql::exec(
        db,
        "UPDATE agent_tasks SET phase = ?2, status = coalesce(?3, status),
          delivery = coalesce(?4, delivery) WHERE id = ?1",
        [
            task.into(),
            phase.as_str().into(),
            status.into(),
            delivery.into(),
        ],
    )
    .await?;
    if status.is_some() {
        sql::exec(
            db,
            "UPDATE agent_tasks SET payload = NULL WHERE id = ?1",
            [task.into()],
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn suppress_descendant_wakes(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
) -> Result<(), Error> {
    let family = family(db, thread, run).await?;
    for task in family
        .tasks
        .into_iter()
        .filter(|task| task.delivery == "prompt")
    {
        cancel_task(db, &task.id, task.phase, Some("entry_only")).await?;
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
    let pending_delivery = family.tasks.iter().any(|task| task.delivery != "delivered");
    let wakes = by_ids(db, thread, &family.wakes).await?;
    Ok(Family::from_runs(root, wakes, pending_delivery))
}

struct FamilyRows {
    tasks: Vec<FamilyTask>,
    wakes: HashSet<RunId>,
}

struct FamilyTask {
    id: TaskId,
    parent: RunId,
    phase: Phase,
    delivery: String,
    wake: Option<RunId>,
}

/// Original result wakes join the family after delivery is recorded. Follow-up
/// wakes join on their receipt, which is persisted with the new run.
async fn family(db: &mut Connection, thread: ThreadId, run: RunId) -> Result<FamilyRows, Error> {
    let tasks = sql::rows(
        db,
        "SELECT id, parent_run, phase, delivery, wake_run FROM agent_tasks
         WHERE parent = ?1 AND parent_run IS NOT NULL",
        [thread.into()],
    )
    .await?
    .map(|mut row| {
        Ok(FamilyTask {
            id: TaskId(row.str(0)?),
            parent: row.id(1)?,
            phase: row.str(2)?.parse().map_err(Error::corrupt)?,
            delivery: row.str(3)?,
            wake: row.opt_id(4)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    let follow_ups = sql::rows(
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
    let original = tasks.iter().filter_map(|task| {
        task.wake.map(|wake| WakeEdge {
            parent: task.parent,
            wake,
        })
    });
    let runs = decisions::descendants(run, original.chain(follow_ups.iter().copied()));
    let tasks: Vec<_> = tasks
        .into_iter()
        .filter(|task| runs.contains(&task.parent))
        .collect();
    let wakes = tasks
        .iter()
        .filter_map(|task| task.wake)
        .chain(
            follow_ups
                .into_iter()
                .filter(|edge| runs.contains(&edge.parent))
                .map(|edge| edge.wake),
        )
        .collect();
    Ok(FamilyRows { tasks, wakes })
}

pub(super) fn parse_status(status: &str) -> Result<TaskStatus, Error> {
    match status {
        "queued" => Ok(TaskStatus::Queued),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        _ => Err(Error::corrupt(format!(
            "invalid persisted task status {status}"
        ))),
    }
}
