//! Thread lifecycles, and the activity triage is decided on.

use viz_core::lifecycle::{Activity, Awaiting};
use viz_protocol::{AutoSettle, Lifecycle, RunStatus, SettledBy, Standing, ThreadId, ThreadStatus};

use super::{
    super::{
        runs,
        sql::{Conn, Row, exec, row, rows},
    },
    now,
};
use crate::error::Error;

/// Columns of a thread `t` for [`decode`].
pub(super) const COLUMNS: &str = "t.settled_at, t.settled_by, t.kept, t.snoozed_until, t.pin, t.sort_order,
    t.auto_settle, t.archived, t.unread, t.updated_at, t.last_input_at,
    EXISTS (SELECT 1 FROM runs WHERE thread = t.id AND status IN ('queued','running')),
    t.setup_failed,
    (SELECT status FROM runs WHERE thread = t.id AND end_entry IS NOT NULL ORDER BY end_entry DESC LIMIT 1),
    (SELECT count(*) FROM agent_tasks WHERE parent = t.id AND status IN ('queued','running')),
    (SELECT json_extract(body, '$.type') FROM run_requests WHERE thread = t.id AND pending ORDER BY id LIMIT 1)";

/// A thread for triage.
#[derive(Debug)]
pub(in crate::workspace) struct Standings {
    pub(in crate::workspace) thread: ThreadId,
    pub(in crate::workspace) cwd: String,
    pub(in crate::workspace) lifecycle: Lifecycle,
    pub(in crate::workspace) activity: Activity,
    pub(in crate::workspace) auto_settle: Option<viz_protocol::ProjectAutoSettle>,
}

/// Read [`COLUMNS`] starting at column `first`.
pub(super) fn decode(
    row: &mut Row,
    first: usize,
) -> Result<(Lifecycle, Activity, ThreadStatus), Error> {
    let standing = match (row.opt_i64(first)?, row.opt_str(first + 1)?) {
        (Some(at), Some(by)) => Standing::Settled {
            at,
            by: match by.as_str() {
                "user" => SettledBy::User,
                "inactivity" => SettledBy::Inactivity,
                "pull_request" => SettledBy::PullRequest,
                by => return Err(Error::corrupt(format_args!("settled by {by}"))),
            },
        },
        _ if row.flag(first + 2)? => Standing::Kept,
        _ => Standing::Open,
    };
    let lifecycle = Lifecycle {
        standing,
        snoozed_until: row.opt_i64(first + 3)?,
        pin: row.opt_i64(first + 4)?,
        order: row.opt_i64(first + 5)?,
        auto_settle: match row.str(first + 6)?.as_str() {
            "follow" => AutoSettle::Follow,
            "never" => AutoSettle::Never,
            mode => return Err(Error::corrupt(format_args!("automatic settlement {mode}"))),
        },
        archived: row.flag(first + 7)?,
        unread: row.flag(first + 8)?,
    };
    let working = row.flag(first + 11)?;
    let last: Option<RunStatus> = row
        .opt_str(first + 13)?
        .as_deref()
        .map(runs::decode_status)
        .transpose()?;
    let status = viz_core::threads::status(working, row.flag(first + 12)?, last);
    let activity = Activity {
        working,
        failed: status == ThreadStatus::Failed,
        awaiting: match row.opt_str(first + 15)?.as_deref() {
            None => None,
            Some("permission") => Some(Awaiting::Approval),
            Some(_) => Some(Awaiting::Question),
        },
        waiting_on: u32::try_from(row.u64(first + 14)?).unwrap_or(u32::MAX),
        updated_at: row.i64(first + 9)?,
        last_input_at: row.opt_i64(first + 10)?,
        pull_request: None,
    };
    Ok((lifecycle, activity, status))
}

pub(in crate::workspace) fn lifecycle(
    db: &Conn,
    thread: ThreadId,
) -> Result<(Lifecycle, Activity), Error> {
    let query = format!("SELECT {COLUMNS} FROM threads AS t WHERE t.id = ?1");
    let mut row = row(db, &query, [thread.into()])?.ok_or(Error::NoThread(thread))?;
    let (lifecycle, activity, _) = decode(&mut row, 0)?;
    Ok((lifecycle, activity))
}

/// Every thread that is not archived.
pub(in crate::workspace) fn standings(db: &Conn) -> Result<Vec<Standings>, Error> {
    let query = format!(
        "SELECT t.id, json_extract(p.settings, '$.autoSettle'), t.cwd, {COLUMNS}
        FROM threads AS t LEFT JOIN projects AS p ON p.path = t.project WHERE t.archived = 0"
    );
    rows(db, &query, [])?
        .map(|mut row| {
            let (lifecycle, activity, _) = decode(&mut row, 3)?;
            Ok(Standings {
                thread: row.id(0)?,
                cwd: row.str(2)?,
                auto_settle: row
                    .opt_str(1)?
                    .as_deref()
                    .map(super::from_json)
                    .transpose()?,
                lifecycle,
                activity,
            })
        })
        .collect()
}

pub(in crate::workspace) fn save_lifecycle(
    db: &Conn,
    thread: ThreadId,
    lifecycle: &Lifecycle,
) -> Result<(), Error> {
    let (settled_at, settled_by, kept) = match lifecycle.standing {
        Standing::Open => (None, None, false),
        Standing::Kept => (None, None, true),
        Standing::Settled { at, by } => {
            let by = match by {
                SettledBy::User => "user",
                SettledBy::Inactivity => "inactivity",
                SettledBy::PullRequest => "pull_request",
            };
            (Some(at), Some(by), false)
        }
    };
    let auto_settle = match lifecycle.auto_settle {
        AutoSettle::Follow => "follow",
        AutoSettle::Never => "never",
    };
    exec(
        db,
        "UPDATE threads SET settled_at = ?2, settled_by = ?3, kept = ?4, snoozed_until = ?5, pin = ?6,
            sort_order = ?7, auto_settle = ?8, archived = ?9, unread = ?10 WHERE id = ?1",
        [
            thread.into(),
            settled_at.into(),
            settled_by.into(),
            kept.into(),
            lifecycle.snoozed_until.into(),
            lifecycle.pin.into(),
            lifecycle.order.into(),
            auto_settle.into(),
            lifecycle.archived.into(),
            lifecycle.unread.into(),
        ],
    )?;
    Ok(())
}

/// Record what an input, a run's end or the agent did to a thread's lifecycle,
/// and when.
pub(in crate::workspace) fn touch(
    db: &Conn,
    thread: ThreadId,
    change: impl FnOnce(&Lifecycle) -> Lifecycle,
) -> Result<(), Error> {
    let (current, _) = lifecycle(db, thread)?;
    let next = change(&current);
    if next != current {
        save_lifecycle(db, thread, &next)?;
    }
    exec(
        db,
        "UPDATE threads SET updated_at = ?2 WHERE id = ?1",
        [thread.into(), now().into()],
    )?;
    Ok(())
}
