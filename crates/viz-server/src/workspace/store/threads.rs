//! Thread bindings, persisted session metadata, and projected summaries.

use serde::{Deserialize, Serialize};
use viz_core::{
    lifecycle as triage,
    placement::Checkout,
    threads::{Metadata, Title},
};
use viz_protocol::{ForkSource, ThreadUpdated};

use super::{
    super::{
        Workspace,
        sql::{Connection, exec, row, rows},
    },
    from_json, json, lifecycle, now,
};
use crate::{
    acp::{Desired, Selection},
    error::Error,
    wire::{
        AttachmentCapabilities, Command, ConfigOption, QueuedInput, ThreadId, ThreadState,
        ThreadSummary, Usage,
    },
};

/// Why a thread continues in a new session whose first prompt carries its
/// earlier turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::workspace) enum Handoff {
    /// It moved away from this directory. Harnesses differ in whether a
    /// restored session follows a new one.
    Moved { from: String },
    /// Forked from another thread; `session` is the source's session to fork
    /// natively, when the harness can.
    Forked {
        #[serde(default)]
        session: Option<String>,
    },
    /// The harness could not reopen the thread's session.
    Reopened,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(in crate::workspace) struct SavedSession {
    pub(in crate::workspace) session_id: Option<String>,
    pub(in crate::workspace) options: Vec<ConfigOption>,
    pub(in crate::workspace) commands: Vec<Command>,
    pub(in crate::workspace) desired: Desired,
    pub(in crate::workspace) images: bool,
    pub(in crate::workspace) usage: Option<Usage>,
    /// The harness's latest title for the session.
    pub(in crate::workspace) title: Option<String>,
    pub(in crate::workspace) selection: Option<Selection>,
    /// The earlier turns the next prompt must carry, and why.
    pub(in crate::workspace) handoff: Option<Handoff>,
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

/// A change of metadata as reported to the client.
pub(in crate::workspace) fn updated(prior: &Metadata, next: &Metadata) -> ThreadUpdated {
    ThreadUpdated {
        title: next.title().to_owned(),
        project: next.project.clone(),
        prior_title: prior.title().to_owned(),
        prior_project: prior.project.clone(),
    }
}

/// A thread's user metadata, and whether it runs in a linked worktree.
pub(in crate::workspace) async fn metadata(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<(Metadata, bool), Error> {
    let mut row = row(
        db,
        "SELECT title, user_title, worktree, project FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    .ok_or(Error::NoThread(thread))?;
    let title = row.str(0)?;
    Ok((
        Metadata {
            title: if row.flag(1)? {
                Title::User(title)
            } else {
                Title::Derived(title)
            },
            project: row.opt_str(3)?,
        },
        row.opt_str(2)?.is_some(),
    ))
}

pub(in crate::workspace) async fn save_metadata(
    db: &mut Connection,
    thread: ThreadId,
    metadata: &Metadata,
) -> Result<(), Error> {
    let (title, user) = match &metadata.title {
        Title::Derived(title) => (title, false),
        Title::User(title) => (title, true),
    };
    exec(
        db,
        "UPDATE threads SET title = ?2, user_title = ?3, project = ?4 WHERE id = ?1",
        [
            thread.into(),
            title.as_str().into(),
            user.into(),
            metadata.project.as_deref().into(),
        ],
    )
    .await?;
    Ok(())
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

pub(in crate::workspace) async fn thread_cwd(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<String, Error> {
    row(db, "SELECT cwd FROM threads WHERE id = ?1", [thread.into()])
        .await?
        .ok_or(Error::NoThread(thread))?
        .str(0)
}

pub(in crate::workspace) async fn thread_title(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<String, Error> {
    row(
        db,
        "SELECT title FROM threads WHERE id = ?1",
        [thread.into()],
    )
    .await?
    .ok_or(Error::NoThread(thread))?
    .str(0)
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
    workspace: &Workspace,
    db: &mut Connection,
    new: &NewThread<'_>,
) -> Result<ThreadId, Error> {
    workspace.admit_checkout(new.checkout)?;
    let row = row(db, "INSERT INTO threads (harness, cwd, project, parent, parent_environment, title, user_title, session, updated_at, worktree)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id", [
        new.harness.into(), new.checkout.cwd.as_str().into(), new.project.into(), new.parent.into(), new.parent_environment.into(),
        new.title.unwrap_or_default().into(), new.title.is_some().into(), json(&new.session)?.into(), now().into(), new.checkout.worktree.as_deref().into(),
    ]).await?.expect("thread INSERT RETURNING yielded one row");
    row.id(0)
}

/// Raw persisted thread record ready to be enriched with worktree Git branches
/// outside the SQLite lock.
pub(in crate::workspace) struct ThreadRecord {
    pub(in crate::workspace) summary: ThreadSummary,
    pub(in crate::workspace) activity: triage::Activity,
    pub(in crate::workspace) worktree_path: Option<String>,
}

pub(in crate::workspace) async fn thread_rows(
    db: &mut Connection,
    archived: Option<bool>,
    project: Option<&str>,
    workspace: &Workspace,
) -> Result<Vec<ThreadRecord>, Error> {
    let query = format!(
        "SELECT t.id, t.title, t.parent, t.parent_environment, t.project, t.harness,
            t.model, t.thinking, t.worktree, t.forked_from, t.forked_at, t.cwd, {}
        FROM threads AS t WHERE (?1 IS NULL OR t.archived = ?1) AND (?2 IS NULL OR t.project = ?2)",
        lifecycle::COLUMNS
    );
    let now = now();
    let records = rows(db, &query, [archived.into(), project.into()]).await?;
    let mut threads = Vec::new();
    for mut row in records {
        let harness = row.str(5)?;
        let (lifecycle, activity, status) = lifecycle::decode(&mut row, 12)?;
        let id = row.id(0)?;
        let shelf = triage::shelf(&lifecycle, &activity, now);
        let worktree_path = row.opt_str(8)?;
        let summary = ThreadSummary {
            id,
            title: row.str(1)?,
            parent: row.opt_id(2)?,
            parent_environment: row.opt_str(3)?,
            forked_from: row
                .opt_id(9)?
                .map(|thread| {
                    Ok::<_, Error>(ForkSource {
                        thread,
                        entry: row.opt_id(10)?,
                    })
                })
                .transpose()?,
            project: row.opt_str(4)?,
            worktree: None,
            pull_request: workspace.pull_summary(id, &row.str(11)?),
            harness_name: workspace.harness_name(&harness).to_owned(),
            harness,
            status,
            shelf,
            attention: triage::attention(&lifecycle, &activity),
            waiting_on: activity.waiting_on,
            last_input_at: activity.last_input_at,
            updated_at: activity.updated_at,
            model: row.opt_str(6)?,
            thinking: row.opt_str(7)?,
            lifecycle,
        };
        threads.push(ThreadRecord {
            summary,
            activity,
            worktree_path,
        });
    }
    Ok(threads)
}

/// Reads persisted thread state under the SQLite lock without touching Git,
/// returning `(state, raw_worktree_path)` so callers resolve the branch after
/// releasing the lock.
pub(in crate::workspace) async fn state_row(
    db: &mut Connection,
    thread: ThreadId,
) -> Result<(ThreadState, Option<String>), Error> {
    let query = format!(
        "SELECT t.harness, t.project, t.cwd, t.session, t.worktree, {} FROM threads AS t WHERE t.id = ?1",
        lifecycle::COLUMNS
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
    let (_, _, status) = lifecycle::decode(&mut row, 5)?;
    let access = super::requests::access(db, thread, &session.options).await?;
    let requests = super::requests::pending_requests(db, thread).await?;
    let worktree_path = row.opt_str(4)?;
    Ok((
        ThreadState {
            harness: row.str(0)?,
            project: row.opt_str(1)?,
            worktree: None,
            cwd: row.str(2)?,
            status,
            options: session.options,
            commands: session.commands,
            attachments: AttachmentCapabilities {
                images: session.images,
            },
            usage: session.usage,
            queue,
            live: None,
            access,
            requests,
        },
        worktree_path,
    ))
}
