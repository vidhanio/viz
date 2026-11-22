//! Record a continuation's outbox rows in the same transaction as its run end.
//! The receiver deduplicates each wake by the continuation's run ID.

use viz_core::tasks::{self, WakeEdge};
use viz_protocol::{RunId, RunKind, RunStatus};

use super::{
    runs,
    sql::{Connection, exec, row, rows},
};
use crate::error::Error;

pub(super) async fn record(db: &mut Connection, thread: u64, run: u64) -> Result<(), Error> {
    let Some(continuation) = runs::run_from(db, thread, run).await? else {
        return Ok(());
    };
    if continuation.kind != RunKind::Unprompted || continuation.status != RunStatus::Completed {
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
    .map(|mut row| Ok::<_, Error>((RunId(row.u64(0)?), runs::decode_status(&row.str(1)?)?)))
    .transpose()?;
    let Some((parent, text)) = tasks::continuation_parent(&continuation, preceding) else {
        return Ok(());
    };
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
            parent: RunId(row.u64(0)?),
            wake: RunId(row.u64(1)?),
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    let ancestry = tasks::ancestors(parent, edges);
    for mut receipt in rows(
        db,
        "SELECT tr.environment, tr.task, tr.run,
            EXISTS(SELECT 1 FROM stopped_task_runs s WHERE s.thread = tr.thread AND s.run = tr.run)
         FROM task_receipts tr WHERE tr.thread = ?1",
        [thread.into()],
    )
    .await?
    {
        if !receipt.flag(3)? && ancestry.contains(&RunId(receipt.u64(2)?)) {
            exec(
                db,
                "INSERT OR IGNORE INTO task_follow_ups (environment, task, run, thread, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                [
                    receipt.str(0)?.into(),
                    receipt.str(1)?.into(),
                    run.into(),
                    thread.into(),
                    text.into(),
                ],
            )
            .await?;
        }
    }
    Ok(())
}
