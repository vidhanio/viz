//! Transcript entries, tool patches, and the full-text search index.

use super::{
    super::sql::{Connection, Row, exec, row, rows},
    from_json, json, require,
};
use crate::{
    acp::Output,
    error::Error,
    wire::{Entry, EntryId, EntryPage, NewEntry, SearchHit, ThreadId, ToolCallStatus},
};

pub(in crate::workspace) async fn publish(
    db: &mut Connection,
    thread: ThreadId,
    session: &str,
    output: Output,
) -> Result<Option<(String, ToolCallStatus)>, Error> {
    let (entry, tool_session, patch) = match output {
        Output::Entry(entry) => {
            let Some(entry) = crate::acp::entry(*entry) else {
                return Ok(None);
            };
            (entry, Some(session), None)
        }
        Output::ToolPatch(report) => {
            let id = &report.id;
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
                let updated =
                    crate::acp::tool(&report.apply(crate::acp::tool_input(record.clone())));
                let patch = Some((updated.id.clone(), updated.status));
                if updated == record {
                    return Ok(patch);
                }
                exec(
                    db,
                    "UPDATE entries SET data = ?2 WHERE id = ?1",
                    [id.into(), json(&NewEntry::Tool(updated.clone()))?.into()],
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
                let call = viz_acp::ToolCall::new(id.clone());
                let record = crate::acp::tool(&report.apply(call));
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

pub(in crate::workspace) async fn append(
    db: &mut Connection,
    thread: ThreadId,
    data: &NewEntry,
) -> Result<EntryId, Error> {
    let id = row(
        db,
        "INSERT INTO entries (thread, data) VALUES (?1, ?2) RETURNING id",
        [thread.into(), json(data)?.into()],
    )
    .await?
    .expect("entry INSERT RETURNING yielded one row")
    .id(0)?;
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

pub(in crate::workspace) async fn entries(
    db: &mut Connection,
    thread: ThreadId,
    before: Option<EntryId>,
    limit: usize,
) -> Result<EntryPage, Error> {
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

pub(in crate::workspace) async fn entries_after(
    db: &mut Connection,
    thread: ThreadId,
    after: Option<EntryId>,
    limit: usize,
) -> Result<Vec<Entry>, Error> {
    require(db, thread).await?;
    rows(
        db,
        "SELECT id, data FROM entries WHERE thread = ?1 AND id > coalesce(?2, 0) ORDER BY id LIMIT ?3",
        [thread.into(), after.into(), limit.into()],
    )
    .await?
    .map(decode_entry)
    .collect()
}

fn decode_entry(mut row: Row) -> Result<Entry, Error> {
    Ok(Entry {
        id: row.id(0)?,
        data: from_json(&row.str(1)?)?,
    })
}

pub(in crate::workspace) async fn search(
    db: &mut Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, Error> {
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
            thread: row.id(0)?,
            entry: row.id(1)?,
            snippet: row.str(2)?,
        })
    })
    .collect()
}
