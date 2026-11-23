//! Record a continuation's outbox rows in the same transaction as its run end.
//! The receiver deduplicates each wake by the continuation's run ID.

use viz_core::tasks::{self, Receipt, WakeEdge};
use viz_protocol::{RunId, RunKind, TaskId, ThreadId};

use super::{
    runs,
    sql::{Connection, exec, row, rows},
};
use crate::error::Error;

pub(super) async fn record(db: &mut Connection, thread: ThreadId, run: RunId) -> Result<(), Error> {
    let Some(continuation) = runs::run_from(db, thread, run).await? else {
        return Ok(());
    };
    if continuation.kind != RunKind::Unprompted {
        return Ok(());
    }
    let preceding = row(
        db,
        "SELECT p.id, p.status FROM runs u JOIN runs p ON p.thread = u.thread
         WHERE u.id = ?1 AND p.kind = 'prompt' AND p.end_entry < u.prompt_entry
         ORDER BY p.end_entry DESC LIMIT 1",
        [run.into()],
    )
    .await?
    .map(|mut row| Ok::<_, Error>((row.id(0)?, runs::decode_status(&row.str(1)?)?)))
    .transpose()?;
    let edges = rows(
        db,
        "SELECT a.parent_run, w.run FROM task_wakes w
         JOIN agent_tasks a ON a.id = w.task AND a.parent = w.thread
         WHERE w.thread = ?1 AND w.run IS NOT NULL AND a.parent_run IS NOT NULL",
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
    let receipts = rows(
        db,
        "SELECT tr.environment, tr.task, tr.run,
            EXISTS(SELECT 1 FROM stopped_task_runs s WHERE s.thread = tr.thread AND s.run = tr.run)
         FROM task_receipts tr WHERE tr.thread = ?1",
        [thread.into()],
    )
    .await?
    .map(|mut row| {
        Ok(Receipt {
            environment: row.str(0)?,
            task: TaskId(row.str(1)?),
            run: row.id(2)?,
            stopped: row.flag(3)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    let Some((text, owed)) = tasks::continued(&continuation, preceding, edges, receipts) else {
        return Ok(());
    };
    for receipt in owed {
        exec(
            db,
            "INSERT OR IGNORE INTO task_follow_ups (environment, task, run, thread, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            [
                receipt.environment.into(),
                receipt.task.into(),
                run.into(),
                thread.into(),
                text.into(),
            ],
        )
        .await?;
    }
    Ok(())
}
