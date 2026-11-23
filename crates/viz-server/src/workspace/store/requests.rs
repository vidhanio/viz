//! Harness requests and the access modes chosen for threads and projects.

use viz_core::requests::{self, Command};
use viz_protocol::{
    Access, AccessMode, RequestBody, RequestId, RequestState, RunId, RunRequest, ThreadId,
    Withdrawal,
};

use super::{
    super::sql::{Connection, Row, exec, row, rows},
    from_json, json, now,
};
use crate::error::Error;

const COLUMNS: &str = "id, thread, run, created_at, body, state";

fn decode(mut row: Row) -> Result<RunRequest, Error> {
    Ok(RunRequest {
        id: row.id(0)?,
        thread: row.id(1)?,
        run: row.id(2)?,
        created_at: row.i64(3)?,
        body: from_json(&row.str(4)?)?,
        state: from_json(&row.str(5)?)?,
    })
}

/// Records a request that waits for an answer.
pub(in crate::workspace) async fn insert_request(
    db: &mut Connection,
    thread: ThreadId,
    run: RunId,
    body: &RequestBody,
) -> Result<RunRequest, Error> {
    let created_at = now();
    let row = row(
        db,
        "INSERT INTO run_requests (thread, run, created_at, body, state) VALUES (?1, ?2, ?3, ?4, ?5)
        RETURNING id",
        [
            thread.into(),
            run.into(),
            created_at.into(),
            json(body)?.into(),
            json(&RequestState::Pending)?.into(),
        ],
    )
    .await?
    .expect("request INSERT RETURNING yielded one row");
    Ok(RunRequest {
        id: row.id(0)?,
        thread,
        run,
        created_at,
        body: body.clone(),
        state: RequestState::Pending,
    })
}

pub(in crate::workspace) async fn request(
    db: &mut Connection,
    thread: ThreadId,
    id: RequestId,
) -> Result<Option<RunRequest>, Error> {
    row(
        db,
        &format!("SELECT {COLUMNS} FROM run_requests WHERE id = ?1 AND thread = ?2"),
        [id.into(), thread.into()],
    )
    .await?
    .map(decode)
    .transpose()
}

/// A thread's requests that still wait, oldest first.
pub(in crate::workspace) async fn pending_requests(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<Vec<RunRequest>, Error> {
    rows(
        db,
        &format!("SELECT {COLUMNS} FROM run_requests WHERE thread = ?1 AND pending ORDER BY id"),
        [thread.into()],
    )
    .await?
    .map(decode)
    .collect()
}

pub(in crate::workspace) async fn save_request(
    db: &mut Connection,
    request: &RunRequest,
) -> Result<(), Error> {
    exec(
        db,
        "UPDATE run_requests SET state = ?2 WHERE id = ?1",
        [request.id.into(), json(&request.state)?.into()],
    )
    .await?;
    Ok(())
}

/// Withdraws the waiting requests of one run, or of every run, and returns
/// the threads they belonged to.
pub(in crate::workspace) async fn withdraw_requests(
    db: &mut Connection,
    run: Option<RunId>,
    reason: Withdrawal,
) -> Result<Vec<ThreadId>, Error> {
    let pending = rows(
        db,
        &format!("SELECT {COLUMNS} FROM run_requests WHERE pending AND (?1 IS NULL OR run = ?1)"),
        [run.into()],
    )
    .await?
    .map(decode)
    .collect::<Result<Vec<_>, Error>>()?;
    let at = now();
    let mut threads = Vec::new();
    for mut request in pending {
        let asker = requests::Asker { parent: None };
        if let Ok(state) = requests::decide(
            &request.state,
            &request.body,
            asker,
            Command::Withdraw(reason),
            at,
        ) {
            request.state = state;
            save_request(db, &request).await?;
            if !threads.contains(&request.thread) {
                threads.push(request.thread);
            }
        }
    }
    Ok(threads)
}

/// The thread's chosen access mode and its project's default for the
/// thread's harness.
pub(in crate::workspace) async fn chosen_access(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<(Option<AccessMode>, Option<AccessMode>), Error> {
    let Some(mut row) = row(
        db,
        "SELECT t.access, p.access FROM threads AS t
        LEFT JOIN project_access AS p ON p.project = t.project AND p.harness = t.harness
        WHERE t.id = ?1",
        [thread.into()],
    )
    .await?
    else {
        return Err(Error::NoThread(thread));
    };
    let thread = row.opt_str(0)?.map(|text| from_json(&text)).transpose()?;
    let project = row.opt_str(1)?.map(|text| from_json(&text)).transpose()?;
    Ok((thread, project))
}

/// The thread's access, from the options its harness last reported.
pub(in crate::workspace) async fn access(
    db: &mut Connection,
    thread: ThreadId,
    options: &[viz_protocol::ConfigOption],
) -> Result<Access, Error> {
    let (chosen, project) = chosen_access(db, thread).await?;
    Ok(requests::access(options, chosen.as_ref(), project.as_ref()))
}

pub(in crate::workspace) async fn save_thread_access(
    db: &mut Connection,
    thread: ThreadId,
    access: Option<&AccessMode>,
) -> Result<(), Error> {
    exec(
        db,
        "UPDATE threads SET access = ?2 WHERE id = ?1",
        [thread.into(), access.map(json).transpose()?.into()],
    )
    .await?;
    Ok(())
}

pub(in crate::workspace) async fn save_project_access(
    db: &mut Connection,
    project: &str,
    harness: &str,
    access: Option<&AccessMode>,
) -> Result<(), Error> {
    match access {
        Some(access) => {
            exec(
                db,
                "INSERT INTO project_access (project, harness, access) VALUES (?1, ?2, ?3)
                ON CONFLICT (project, harness) DO UPDATE SET access = excluded.access",
                [project.into(), harness.into(), json(access)?.into()],
            )
            .await?;
        }
        None => {
            exec(
                db,
                "DELETE FROM project_access WHERE project = ?1 AND harness = ?2",
                [project.into(), harness.into()],
            )
            .await?;
        }
    }
    Ok(())
}
