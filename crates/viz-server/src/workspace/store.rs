use std::{
    env,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use toasty::Executor;

use super::{
    Workspace,
    sql::{exec, row, rows},
};
use crate::{
    acp::{Desired, Selection, tool_record::ToolContent, transcript::Output},
    wire::{
        AttachmentCapabilities, Command, ConfigOption, Entry, EntryId, EntryPage, NewEntry,
        Project, QueuedInput, RpcError, RunId, SearchHit, ThreadId, ThreadState, ThreadStatus,
        ThreadSummary, ToolCallStatus, Usage,
    },
};

pub(super) fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, RpcError> {
    serde_json::to_string(value).map_err(RpcError::internal)
}

pub(super) fn from_json<T: DeserializeOwned>(value: &str) -> Result<T, RpcError> {
    serde_json::from_str(value).map_err(RpcError::internal)
}

pub(super) fn now() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

pub(super) async fn directory(path: &Path) -> Result<PathBuf, RpcError> {
    let path = if let Ok(rest) = path.strip_prefix("~") {
        PathBuf::from(env::var_os("HOME").ok_or_else(|| RpcError::invalid("HOME is not set"))?)
            .join(rest)
    } else {
        path.to_path_buf()
    };
    let canonical = tokio::fs::canonicalize(&path)
        .await
        .map_err(|e| RpcError::invalid(format!("cannot read directory {}: {e}", path.display())))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(RpcError::internal)?;
    if !metadata.is_dir() {
        return Err(RpcError::invalid(format!(
            "not a directory: {}",
            path.display()
        )));
    }
    Ok(canonical)
}

pub(super) fn path_text(path: &Path) -> Result<&str, RpcError> {
    path.to_str()
        .ok_or_else(|| RpcError::invalid("working directory is not UTF-8"))
}

pub(super) async fn project(db: &mut dyn Executor, path: &str) -> Result<Project, RpcError> {
    let mut row = row(
        db,
        "SELECT name, created_at FROM projects WHERE path = ?1",
        [path.into()],
    )
    .await?
    .ok_or_else(|| RpcError::not_found(format!("no project {path}")))?;
    Ok(Project {
        path: path.to_owned(),
        name: row.str(0)?,
        created_at: row.i64(1)?,
    })
}

pub(super) async fn projects(db: &mut dyn Executor) -> Result<Vec<Project>, RpcError> {
    rows(
        db,
        "SELECT path, name, created_at FROM projects ORDER BY name, path",
        [],
    )
    .await?
    .map(|mut row| {
        Ok(Project {
            path: row.str(0)?,
            name: row.str(1)?,
            created_at: row.i64(2)?,
        })
    })
    .collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct SavedSession {
    pub(super) session_id: Option<String>,
    pub(super) options: Vec<ConfigOption>,
    pub(super) commands: Vec<Command>,
    pub(super) desired: Desired,
    pub(super) images: bool,
    pub(super) usage: Option<Usage>,
    pub(super) selection: Option<Selection>,
}

#[derive(Debug)]
pub(super) struct Binding {
    pub(super) harness: String,
    pub(super) cwd: String,
    pub(super) project: Option<String>,
    pub(super) parent: Option<ThreadId>,
    pub(super) parent_environment: Option<String>,
    pub(super) session: SavedSession,
}

pub(super) async fn binding(db: &mut dyn Executor, thread: u64) -> Result<Binding, RpcError> {
    let mut row = row(db, "SELECT harness, cwd, project, parent, parent_environment, session FROM threads WHERE id = ?1", [thread.into()]).await?
        .ok_or_else(|| missing(thread))?;
    Ok(Binding {
        harness: row.str(0)?,
        cwd: row.str(1)?,
        project: row.opt_str(2)?,
        parent: row.opt_u64(3)?.map(ThreadId),
        parent_environment: row.opt_str(4)?,
        session: from_json(&row.str(5)?)?,
    })
}

pub(super) async fn require(db: &mut dyn Executor, thread: u64) -> Result<(), RpcError> {
    row(db, "SELECT id FROM threads WHERE id = ?1", [thread.into()])
        .await?
        .map(|_| ())
        .ok_or_else(|| missing(thread))
}

pub(super) fn missing(thread: u64) -> RpcError {
    RpcError::not_found(format!("no thread {thread}"))
}

pub(super) struct NewThread<'a> {
    pub(super) harness: &'a str,
    pub(super) cwd: &'a str,
    pub(super) project: Option<&'a str>,
    pub(super) parent: Option<u64>,
    pub(super) parent_environment: Option<&'a str>,
    pub(super) title: Option<&'a str>,
    pub(super) session: SavedSession,
}

pub(super) async fn create_thread(
    db: &mut dyn Executor,
    new: &NewThread<'_>,
) -> Result<u64, RpcError> {
    let row = row(db, "INSERT INTO threads (harness, cwd, project, parent, parent_environment, title, user_title, session, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id", [
        new.harness.into(), new.cwd.into(), new.project.into(), new.parent.into(), new.parent_environment.into(),
        new.title.unwrap_or_default().into(), new.title.is_some().into(), json(&new.session)?.into(), now().into(),
    ]).await?.expect("thread INSERT RETURNING yielded one row");
    row.u64(0)
}

const STATUS: &str = "CASE
    WHEN EXISTS (SELECT 1 FROM runs WHERE thread = t.id AND status IN ('queued','running')) THEN 'working'
    WHEN t.setup_failed = 1 OR (SELECT status FROM runs WHERE thread = t.id AND end_entry IS NOT NULL
           ORDER BY end_entry DESC LIMIT 1) IN ('failed','interrupted') THEN 'failed'
    ELSE 'idle' END";

pub(super) async fn list_threads(
    db: &mut dyn Executor,
    archived: Option<bool>,
    project: Option<&str>,
    workspace: &Workspace,
) -> Result<Vec<ThreadSummary>, RpcError> {
    let query = format!(
        "SELECT t.id, t.title, t.parent, t.parent_environment, t.project, t.harness, {STATUS},
            t.archived, t.updated_at, t.unread, t.pinned, t.model, t.thinking
        FROM threads AS t WHERE (?1 IS NULL OR t.archived = ?1) AND (?2 IS NULL OR t.project = ?2)
        ORDER BY t.pinned DESC, t.updated_at DESC, t.id DESC"
    );
    rows(db, &query, [archived.into(), project.into()])
        .await?
        .map(|mut row| {
            let harness = row.str(5)?;
            Ok(ThreadSummary {
                id: ThreadId(row.u64(0)?),
                title: row.str(1)?,
                parent: row.opt_u64(2)?.map(ThreadId),
                parent_environment: row.opt_str(3)?,
                project: row.opt_str(4)?,
                harness_name: workspace.harness_name(&harness).to_owned(),
                harness,
                status: status(&row.str(6)?)?,
                archived: row.flag(7)?,
                updated_at: row.i64(8)?,
                unread: row.flag(9)?,
                pinned: row.flag(10)?,
                model: row.opt_str(11)?,
                thinking: row.opt_str(12)?,
            })
        })
        .collect()
}

pub(super) async fn state(db: &mut dyn Executor, thread: u64) -> Result<ThreadState, RpcError> {
    let query = format!(
        "SELECT t.harness, t.project, t.cwd, t.session, {STATUS} FROM threads AS t WHERE t.id = ?1"
    );
    let mut row = row(db, &query, [thread.into()])
        .await?
        .ok_or_else(|| missing(thread))?;
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
            run: RunId(row.u64(0)?),
            text: row.str(1)?,
            attachment_count: row.u64(2)?,
        })
    })
    .collect::<Result<_, RpcError>>()?;
    Ok(ThreadState {
        harness: row.str(0)?,
        project: row.opt_str(1)?,
        cwd: row.str(2)?,
        status: status(&row.str(4)?)?,
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

fn status(value: &str) -> Result<ThreadStatus, RpcError> {
    Ok(match value {
        "idle" => ThreadStatus::Idle,
        "working" => ThreadStatus::Working,
        "failed" => ThreadStatus::Failed,
        _ => {
            return Err(RpcError::internal(format_args!(
                "unknown thread status {value}"
            )));
        }
    })
}

pub(super) async fn publish(
    db: &mut dyn Executor,
    thread: u64,
    session: &str,
    output: Output,
) -> Result<Option<(String, ToolCallStatus)>, RpcError> {
    let (entry, tool_session, patch) = match output {
        Output::Entry(entry) => (*entry, Some(session), None),
        Output::ToolPatch {
            id,
            fields,
            receipts,
        } => {
            let previous = row(
                db,
                "SELECT id, data FROM entries WHERE thread = ?1 AND tool_session = ?2
                 AND json_extract(data, '$.data.id') = ?3 ORDER BY id DESC LIMIT 1",
                [thread.into(), session.into(), id.as_str().into()],
            )
            .await?
            .map(decode_entry)
            .transpose()?;
            if let Some(Entry {
                id,
                data: NewEntry::Tool(record),
            }) = previous
            {
                let updated = ToolContent::from_record(record.clone())
                    .update(*fields, &receipts)
                    .clone();
                let patch = Some((updated.id.clone(), updated.status));
                if updated == record {
                    return Ok(patch);
                }
                exec(
                    db,
                    "UPDATE entries SET data = ?2 WHERE id = ?1",
                    [id.0.into(), json(&NewEntry::Tool(updated.clone()))?.into()],
                )
                .await?;
                (
                    NewEntry::ToolUpdate {
                        entry: id,
                        record: updated,
                    },
                    None,
                    patch,
                )
            } else {
                let record = ToolContent::new(id).update(*fields, &receipts).clone();
                let patch = Some((record.id.clone(), record.status));
                (NewEntry::Tool(record), Some(session), patch)
            }
        }
    };
    let id = append(db, thread, &entry).await?;
    if let (NewEntry::Tool(_), Some(session)) = (entry, tool_session) {
        exec(
            db,
            "UPDATE entries SET tool_session = ?2 WHERE id = ?1",
            [id.into(), session.into()],
        )
        .await?;
    }
    Ok(patch)
}

pub(super) async fn append(
    db: &mut dyn Executor,
    thread: u64,
    data: &NewEntry,
) -> Result<u64, RpcError> {
    let id = row(
        db,
        "INSERT INTO entries (thread, data) VALUES (?1, ?2) RETURNING id",
        [thread.into(), json(data)?.into()],
    )
    .await?
    .expect("entry INSERT RETURNING yielded one row")
    .u64(0)?;
    let text = match data {
        NewEntry::Prompt { text, .. }
        | NewEntry::Text { text, .. }
        | NewEntry::Thought { text, .. }
        | NewEntry::TaskResult { text, .. } => text.as_str(),
        NewEntry::Tool(tool) => &tool.title,
        NewEntry::Run(_)
        | NewEntry::Plan { .. }
        | NewEntry::Continued
        | NewEntry::ToolUpdate { .. } => return Ok(id),
    };
    if !text.trim().is_empty() {
        let text = if text.contains(['\0', '\u{1}', '\u{2}']) {
            text.replace(['\0', '\u{1}', '\u{2}'], " ").into()
        } else {
            text.into()
        };
        exec(
            db,
            "INSERT INTO search_entries (rowid, thread, text) VALUES (?1, ?2, ?3)",
            [id.into(), thread.into(), text],
        )
        .await?;
    }
    Ok(id)
}

pub(super) async fn entries(
    db: &mut dyn Executor,
    thread: u64,
    before: Option<u64>,
    limit: usize,
) -> Result<EntryPage, RpcError> {
    require(db, thread).await?;
    let mut entries = if let Some(before) = before {
        rows(
            db,
            "SELECT id, data FROM entries WHERE thread = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
            [thread.into(), before.into(), (limit + 1).into()],
        )
        .await?
        .map(decode_entry)
        .collect::<Result<Vec<_>, _>>()?
    } else {
        rows(
            db,
            "SELECT id, data FROM entries WHERE thread = ?1 ORDER BY id DESC LIMIT ?2",
            [thread.into(), (limit + 1).into()],
        )
        .await?
        .map(decode_entry)
        .collect::<Result<Vec<_>, _>>()?
    };
    let before = if entries.len() > limit {
        entries.pop();
        entries.last().map(|entry| entry.id)
    } else {
        None
    };
    entries.reverse();
    Ok(EntryPage { entries, before })
}

pub(super) async fn entries_after(
    db: &mut dyn Executor,
    thread: u64,
    after: u64,
    limit: usize,
) -> Result<Vec<Entry>, RpcError> {
    require(db, thread).await?;
    rows(
        db,
        "SELECT id, data FROM entries WHERE thread = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
        [thread.into(), after.into(), limit.into()],
    )
    .await?
    .map(decode_entry)
    .collect()
}

fn decode_entry(mut row: super::sql::Row) -> Result<Entry, RpcError> {
    Ok(Entry {
        id: EntryId(row.u64(0)?),
        data: from_json(&row.str(1)?)?,
    })
}

pub(super) async fn search(
    db: &mut dyn Executor,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, RpcError> {
    let mut terms = String::new();
    for term in query
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|term| !term.is_empty())
    {
        if !terms.is_empty() {
            terms.push(' ');
        }
        terms.push('"');
        terms.push_str(term);
        terms.push('"');
    }
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    rows(
        db,
        "SELECT s.thread, s.rowid, snippet(search_entries, 0, char(1), char(2), '…', 32)
        FROM search_entries AS s
        WHERE search_entries MATCH ?1 ORDER BY bm25(search_entries), s.rowid DESC LIMIT ?2",
        [terms.into(), limit.into()],
    )
    .await?
    .map(|mut row| {
        Ok(SearchHit {
            thread: ThreadId(row.u64(0)?),
            entry: EntryId(row.u64(1)?),
            snippet: row.str(2)?,
        })
    })
    .collect()
}
