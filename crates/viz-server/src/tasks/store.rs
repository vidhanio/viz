//! SQLite persistence for tasks and stopped run families. Phase decisions
//! consume the committed row under the workspace write lease.

use viz_core::tasks::{self as decisions, Command, Family, Phase, Transition};

use crate::{
    wire::{RpcError, RunId, RunStatus, TaskStatus},
    workspace::{
        run_from,
        sql::{self, Connection},
    },
};

/// A delegated run includes only the descendants created in that run and the
/// runs woken by those descendants, never manually queued follow-ups.
macro_rules! family_sql {
    ($query:expr) => {
        concat!(
            "WITH RECURSIVE lineage(id) AS (SELECT ?2 UNION SELECT a.wake_run ",
            "FROM agent_tasks a JOIN lineage p ON a.parent_run = p.id ",
            "WHERE a.parent = ?1 AND a.wake_run IS NOT NULL UNION SELECT w.run ",
            "FROM agent_tasks a JOIN lineage p ON a.parent_run = p.id ",
            "JOIN task_wakes w ON w.thread = a.parent AND w.task = a.id ",
            "WHERE a.parent = ?1 AND w.sequence > 0 AND w.run IS NOT NULL) ",
            $query,
        )
    };
}

pub(super) async fn prior_request(
    db: &mut Connection,
    parent: u64,
    request: &str,
) -> Result<Option<String>, RpcError> {
    sql::row(
        db,
        "SELECT id FROM agent_tasks WHERE parent = ?1 AND request_id = ?2",
        [parent.into(), request.into()],
    )
    .await?
    .map(|mut row| row.str(0))
    .transpose()
}

pub(super) async fn stop_exists(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<bool, RpcError> {
    Ok(sql::row(
        db,
        "SELECT handled FROM stopped_task_runs WHERE thread = ?1 AND run = ?2",
        [thread.into(), run.into()],
    )
    .await?
    .is_some())
}

pub(super) async fn task_phase(db: &mut Connection, task: &str) -> Result<Option<Phase>, RpcError> {
    sql::row(
        db,
        "SELECT phase FROM agent_tasks WHERE id = ?1",
        [task.into()],
    )
    .await?
    .map(|mut row| row.str(0)?.parse::<Phase>().map_err(RpcError::internal))
    .transpose()
}

pub(super) async fn finish_task(
    db: &mut Connection,
    task: &str,
    status: TaskStatus,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<(), RpcError> {
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
    task: &str,
    phase: Phase,
    delivery: Option<&str>,
) -> Result<(), RpcError> {
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
    thread: u64,
    run: u64,
) -> Result<(), RpcError> {
    let descendants = sql::rows(
        db,
        family_sql!(
            "SELECT id, phase FROM agent_tasks WHERE parent = ?1
            AND parent_run IN (SELECT id FROM lineage) AND delivery = 'prompt'"
        ),
        [thread.into(), run.into()],
    )
    .await?;
    for mut record in descendants {
        let phase = record
            .str(1)?
            .parse::<Phase>()
            .map_err(RpcError::internal)?;
        cancel_task(db, &record.str(0)?, phase, Some("entry_only")).await?;
    }
    Ok(())
}

pub(super) async fn snapshot(
    db: &mut Connection,
    thread: u64,
    run: u64,
) -> Result<Family, RpcError> {
    let mut root = run_from(db, thread, run).await?;
    let rows = sql::rows(
        db,
        family_sql!(
            "SELECT a.delivery = 'delivered', r.id, r.status FROM agent_tasks a \
         LEFT JOIN runs r ON r.id = a.wake_run AND r.thread = a.parent \
         WHERE a.parent = ?1 AND a.parent_run IN (SELECT id FROM lineage) \
         UNION ALL SELECT 1, r.id, r.status FROM task_wakes w \
         JOIN runs r ON r.id = w.run WHERE w.thread = ?1 AND w.sequence > 0 \
         AND r.id IN (SELECT id FROM lineage) ORDER BY 2 DESC"
        ),
        [thread.into(), run.into()],
    )
    .await?;
    let mut active = Vec::new();
    let mut cancelled_run = root
        .as_ref()
        .is_some_and(|root| root.status == RunStatus::Cancelled);
    if root
        .as_ref()
        .is_some_and(|root| viz_core::runs::is_open(root.status))
    {
        active.push(RunId(run));
    }
    let mut latest = None;
    let mut pending_delivery = false;
    for mut row in rows {
        pending_delivery |= !row.flag(0)?;
        if let Some(wake) = row.opt_u64(1)? {
            let status = row.str(2)?;
            if matches!(status.as_str(), "queued" | "running") {
                active.push(RunId(wake));
            }
            cancelled_run |= status == "cancelled";
            latest = latest.or(Some(wake));
        }
    }
    if active.is_empty()
        && !pending_delivery
        && !cancelled_run
        && let (Some(outcome), Some(latest)) = (&mut root, latest)
        && let Some(settled) = run_from(db, thread, latest).await?
    {
        outcome.status = settled.status;
        outcome.answer = settled.answer;
        outcome.error = settled.error;
    }
    Ok(Family {
        outcome: root,
        active,
        pending_delivery,
        cancelled_run,
    })
}

pub(super) fn parse_status(status: &str) -> Result<TaskStatus, RpcError> {
    match status {
        "queued" => Ok(TaskStatus::Queued),
        "running" => Ok(TaskStatus::Running),
        "completed" => Ok(TaskStatus::Completed),
        "failed" => Ok(TaskStatus::Failed),
        "cancelled" => Ok(TaskStatus::Cancelled),
        _ => Err(RpcError::internal(format!(
            "invalid persisted task status {status}"
        ))),
    }
}
