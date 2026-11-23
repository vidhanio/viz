//! Thread bindings, persisted session metadata, and projected summaries.

use serde::{Deserialize, Serialize};
use viz_core::{placement::Checkout, threads};

use super::{
    super::{
        Workspace, runs,
        sql::{Connection, Row, row, rows},
    },
    from_json, json, now, thread_worktree,
};
use crate::{
    acp::{Desired, Selection},
    error::Error,
    wire::{
        AttachmentCapabilities, Command, ConfigOption, QueuedInput, ThreadId, ThreadState,
        ThreadStatus, ThreadSummary, Usage,
    },
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(in crate::workspace) struct SavedSession {
    pub(in crate::workspace) session_id: Option<String>,
    pub(in crate::workspace) options: Vec<ConfigOption>,
    pub(in crate::workspace) commands: Vec<Command>,
    pub(in crate::workspace) desired: Desired,
    pub(in crate::workspace) images: bool,
    pub(in crate::workspace) usage: Option<Usage>,
    pub(in crate::workspace) selection: Option<Selection>,
    /// Set when the thread moved away from this working directory since
    /// its last prompt. Harnesses differ in whether a restored session
    /// follows a new directory, so a move starts a new session and its first
    /// prompt carries the earlier turns.
    pub(in crate::workspace) handoff: Option<String>,
}

#[derive(Debug)]
pub(in crate::workspace) struct Binding {
    pub(in crate::workspace) harness: String,
    pub(in crate::workspace) checkout: Checkout,
    pub(in crate::workspace) project: Option<String>,
    pub(in crate::workspace) parent: Option<ThreadId>,
    pub(in crate::workspace) parent_environment: Option<String>,
    pub(in crate::workspace) session: SavedSession,
}

pub(in crate::workspace) async fn binding(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<Binding, Error> {
    let mut row = row(db, "SELECT harness, cwd, project, worktree, parent, parent_environment, session FROM threads WHERE id = ?1", [thread.into()]).await?
        .ok_or_else(|| Error::NoThread(thread))?;
    Ok(Binding {
        harness: row.str(0)?,
        checkout: Checkout {
            cwd: row.str(1)?,
            worktree: row.opt_str(3)?,
        },
        project: row.opt_str(2)?,
        parent: row.opt_id(4)?,
        parent_environment: row.opt_str(5)?,
        session: from_json(&row.str(6)?)?,
    })
}

pub(in crate::workspace) async fn require(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<(), Error> {
    row(db, "SELECT id FROM threads WHERE id = ?1", [thread.into()])
        .await?
        .map(|_| ())
        .ok_or_else(|| Error::NoThread(thread))
}

pub(in crate::workspace) struct NewThread<'a> {
    pub(in crate::workspace) harness: &'a str,
    pub(in crate::workspace) checkout: &'a Checkout,
    pub(in crate::workspace) project: Option<&'a str>,
    pub(in crate::workspace) parent: Option<ThreadId>,
    pub(in crate::workspace) parent_environment: Option<&'a str>,
    pub(in crate::workspace) title: Option<&'a str>,
    pub(in crate::workspace) session: SavedSession,
}

pub(in crate::workspace) async fn create_thread(
    db: &mut Connection,
    new: &NewThread<'_>,
) -> Result<ThreadId, Error> {
    let row = row(db, "INSERT INTO threads (harness, cwd, project, parent, parent_environment, title, user_title, session, updated_at, worktree)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id", [
        new.harness.into(), new.checkout.cwd.as_str().into(), new.project.into(), new.parent.into(), new.parent_environment.into(),
        new.title.unwrap_or_default().into(), new.title.is_some().into(), json(&new.session)?.into(), now().into(), new.checkout.worktree.as_deref().into(),
    ]).await?.expect("thread INSERT RETURNING yielded one row");
    row.id(0)
}

const STATUS_COLUMNS: &str = "EXISTS (SELECT 1 FROM runs WHERE thread = t.id AND status IN ('queued','running')),
    t.setup_failed, (SELECT status FROM runs WHERE thread = t.id AND end_entry IS NOT NULL ORDER BY end_entry DESC LIMIT 1)";

pub(in crate::workspace) async fn list_threads(
    db: &mut Connection,
    archived: Option<bool>,
    project: Option<&str>,
    workspace: &Workspace,
) -> Result<Vec<ThreadSummary>, Error> {
    let query = format!(
        "SELECT t.id, t.title, t.parent, t.parent_environment, t.project, t.harness,
            t.archived, t.updated_at, t.unread, t.pinned, t.model, t.thinking, t.worktree, {STATUS_COLUMNS}
        FROM threads AS t WHERE (?1 IS NULL OR t.archived = ?1) AND (?2 IS NULL OR t.project = ?2)
        ORDER BY t.pinned DESC, t.updated_at DESC, t.id DESC"
    );
    rows(db, &query, [archived.into(), project.into()])
        .await?
        .map(|mut row| {
            let harness = row.str(5)?;
            Ok(ThreadSummary {
                id: row.id(0)?,
                title: row.str(1)?,
                parent: row.opt_id(2)?,
                parent_environment: row.opt_str(3)?,
                project: row.opt_str(4)?,
                worktree: row.opt_str(12)?.map(thread_worktree),
                harness_name: workspace.harness_name(&harness).to_owned(),
                harness,
                status: status(&mut row, 13)?,
                archived: row.flag(6)?,
                updated_at: row.i64(7)?,
                unread: row.flag(8)?,
                pinned: row.flag(9)?,
                model: row.opt_str(10)?,
                thinking: row.opt_str(11)?,
            })
        })
        .collect()
}

pub(in crate::workspace) async fn state(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<ThreadState, Error> {
    let query = format!(
        "SELECT t.harness, t.project, t.cwd, t.session, t.worktree, {STATUS_COLUMNS} FROM threads AS t WHERE t.id = ?1"
    );
    let mut row = row(db, &query, [thread.into()])
        .await?
        .ok_or_else(|| Error::NoThread(thread))?;
    let session: SavedSession = from_json(&row.str(3)?)?;
    let queue = rows(
        db,
        "SELECT id, text, coalesce(json_array_length(attachments), 0) FROM runs
        WHERE thread = ?1 AND status = 'queued' ORDER BY position, id",
        [thread.into()],
    )
    .await?
    .map(|mut row| {
        Ok(QueuedInput {
            run: row.id(0)?,
            text: row.str(1)?,
            attachment_count: row.u64(2)?,
        })
    })
    .collect::<Result<_, Error>>()?;
    Ok(ThreadState {
        harness: row.str(0)?,
        project: row.opt_str(1)?,
        worktree: row.opt_str(4)?.map(thread_worktree),
        cwd: row.str(2)?,
        status: status(&mut row, 5)?,
        options: session.options,
        commands: session.commands,
        attachments: AttachmentCapabilities {
            images: session.images,
        },
        usage: session.usage,
        queue,
        live: None,
    })
}

fn status(row: &mut Row, first: usize) -> Result<ThreadStatus, Error> {
    let last = row.opt_str(first + 2)?;
    let last = last.as_deref().map(runs::decode_status).transpose()?;
    Ok(threads::status(
        row.flag(first)?,
        row.flag(first + 1)?,
        last,
    ))
}
