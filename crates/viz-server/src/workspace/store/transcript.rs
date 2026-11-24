//! Transcript entries, tool patches, turn-aligned anchored pagination, and the
//! full-text search index.

use viz_protocol::{Body, EntryAnchor, ToolCall, TranscriptEntry, limits};

use super::{
    super::sql::{Conn, Row, exec, row, rows},
    entry_wire::preview,
    from_json, json, require,
};
use crate::{
    acp::Output,
    error::Error,
    wire::{Entry, EntryId, EntryPage, NewEntry, SearchHit, ThreadId, ToolCallStatus},
};

pub(in crate::workspace) fn publish(
    db: &Conn,
    thread: ThreadId,
    session: &str,
    output: Output,
) -> Result<Option<(String, ToolCallStatus)>, Error> {
    let (entry, patch) = match output {
        Output::Entry(entry) => (NewEntry::Transcript(*entry), None),
        Output::ToolPatch(report) => {
            let report_key = report.key.clone();
            let report_order = report.order;
            let previous = row(
                db,
                "SELECT id, data FROM entries WHERE thread = ?1 AND tool_session = ?2
                 AND json_extract(data, '$.data.body.data.id') = ?3 ORDER BY id DESC LIMIT 1",
                [thread.into(), session.into(), report.id.as_str().into()],
            )?
            .map(decode_raw_entry)
            .transpose()?;
            if let Some(Entry {
                id,
                data: NewEntry::Transcript(mut original),
                ..
            }) = previous
                && let Body::Tool(record) = &mut original.body
            {
                let updated = report.apply((**record).clone());
                let patch = Some((updated.id.clone(), updated.status));
                if updated == **record {
                    return Ok(patch);
                }
                let title_changed = updated.title != record.title;
                **record = updated.clone();
                exec(
                    db,
                    "UPDATE entries SET data = ?2, rev = rev + 1 WHERE id = ?1",
                    [id.into(), json(&NewEntry::Transcript(original))?.into()],
                )?;
                if title_changed {
                    exec(
                        db,
                        "DELETE FROM search_entries WHERE rowid = ?1",
                        [id.into()],
                    )?;
                    index(db, thread, id, &updated.title)?;
                }
                (
                    NewEntry::ToolUpdate {
                        entry: id,
                        record: Box::new(updated),
                    },
                    patch,
                )
            } else {
                let call = ToolCall::new(report.id.clone());
                let record = report.apply(call);
                let patch = Some((record.id.clone(), record.status));
                let entry = TranscriptEntry {
                    key: report_key,
                    order: report_order,
                    body: Body::Tool(Box::new(record)),
                };
                (NewEntry::Transcript(entry), patch)
            }
        }
    };
    let id = append(db, thread, &entry)?;
    if let NewEntry::Transcript(TranscriptEntry {
        body: Body::Tool(_),
        ..
    }) = entry
    {
        exec(
            db,
            "UPDATE entries SET tool_session = ?2 WHERE id = ?1",
            [id.into(), session.into()],
        )?;
    }
    Ok(patch)
}

pub(in crate::workspace) fn append(
    db: &Conn,
    thread: ThreadId,
    data: &NewEntry,
) -> Result<EntryId, Error> {
    let id = row(
        db,
        "INSERT INTO entries (thread, data) VALUES (?1, ?2) RETURNING id",
        [thread.into(), json(data)?.into()],
    )?
    .expect("entry INSERT RETURNING yielded one row")
    .id(0)?;
    let text = match data {
        NewEntry::Prompt { prompt, .. } => &prompt.text,
        NewEntry::TaskResult { text, .. } => text,
        NewEntry::Transcript(entry) => match &entry.body {
            Body::Text(segment) | Body::Thought(segment) => &segment.text,
            Body::Tool(tool) => &tool.title,
            Body::Compaction(compaction) => &compaction.summary,
            Body::Plan(_) | Body::Media(_) => return Ok(id),
        },
        _ => return Ok(id),
    };
    index(db, thread, id, text)?;
    Ok(id)
}

fn index(db: &Conn, thread: ThreadId, id: EntryId, text: &str) -> Result<(), Error> {
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
        )?;
    }
    Ok(())
}

pub(in crate::workspace) fn latest_entry_id(
    db: &Conn,
    thread: ThreadId,
) -> Result<Option<EntryId>, Error> {
    let Some(found) = row(
        db,
        "SELECT id FROM entries WHERE thread = ?1 ORDER BY id DESC LIMIT 1",
        [thread.into()],
    )?
    else {
        return Ok(None);
    };
    Ok(Some(found.id(0)?))
}

/// Reads a single stored entry without wire projection trimming (used by MCP
/// `thread_read` whole-entry inspection).
pub(in crate::workspace) fn read_entry_raw(
    db: &Conn,
    thread: ThreadId,
    entry: EntryId,
) -> Result<Entry, Error> {
    row(
        db,
        "SELECT id, data FROM entries WHERE thread = ?1 AND id = ?2",
        [thread.into(), entry.into()],
    )?
    .map(decode_raw_entry)
    .transpose()?
    .ok_or_else(|| Error::not_found(format!("no entry {entry} in thread {thread}")))
}

/// Reads an anchored, turn-aligned page of projected entries bounded safely
/// below [`limits::MAX_WS_FRAME`].
pub(in crate::workspace) fn entries(
    db: &Conn,
    thread: ThreadId,
    anchor: EntryAnchor,
    limit: usize,
    byte_budget: usize,
) -> Result<EntryPage, Error> {
    let budget = byte_budget.min(limits::PAGE_WIRE_BYTES);
    require(db, thread)?;
    match anchor {
        EntryAnchor::Latest => page_backward(db, thread, None, limit, budget),
        EntryAnchor::Before { entry } => page_backward(db, thread, Some(entry), limit, budget),
        EntryAnchor::After { entry } => page_forward(db, thread, entry, limit, budget),
        EntryAnchor::Around { entry } => page_around(db, thread, entry, limit, budget),
    }
}

fn page_backward(
    db: &Conn,
    thread: ThreadId,
    before: Option<EntryId>,
    limit: usize,
    budget: usize,
) -> Result<EntryPage, Error> {
    let fetched = if let Some(before) = before {
        rows(
            db,
            "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
             WHERE thread = ?1 AND id < ?2 AND json_extract(data, '$.kind') != 'tool_update'
             ORDER BY id DESC LIMIT ?3",
            [thread.into(), before.into(), (limit + 1).into()],
        )?
        .map(decode_sized_entry)
        .collect::<Result<Vec<_>, _>>()?
    } else {
        rows(
            db,
            "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
             WHERE thread = ?1 AND json_extract(data, '$.kind') != 'tool_update'
             ORDER BY id DESC LIMIT ?2",
            [thread.into(), (limit + 1).into()],
        )?
        .map(decode_sized_entry)
        .collect::<Result<Vec<_>, _>>()?
    };

    let mut entries = Vec::with_capacity(fetched.len().min(limit));
    let mut bytes = 0usize;
    let mut has_older = fetched.len() > limit;
    for (mut entry, stored_bytes) in fetched.into_iter().take(limit) {
        let size = preview(&mut entry, stored_bytes);
        let needed = size.saturating_add(usize::from(!entries.is_empty()));
        if !entries.is_empty() && bytes.saturating_add(needed) > budget {
            has_older = true;
            break;
        }
        bytes = bytes.saturating_add(needed);
        entries.push(entry);
    }

    if has_older
        && entries.last().is_some_and(|oldest| !is_turn_start(oldest))
        && let Some(cut) = entries.iter().rposition(is_turn_start)
    {
        entries.truncate(cut + 1);
    }

    entries.reverse();
    let older_cursor = if has_older {
        entries.first().map(|entry| entry.id)
    } else {
        None
    };
    let newer_cursor = if before.is_some()
        && let Some(last) = entries.last()
        && has_visible_after(db, thread, last.id)?
    {
        Some(last.id)
    } else {
        None
    };
    Ok(EntryPage {
        entries,
        before: older_cursor,
        after: newer_cursor,
    })
}

fn page_forward(
    db: &Conn,
    thread: ThreadId,
    after: EntryId,
    limit: usize,
    budget: usize,
) -> Result<EntryPage, Error> {
    let fetched = rows(
        db,
        "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
         WHERE thread = ?1 AND id > ?2 AND json_extract(data, '$.kind') != 'tool_update'
         ORDER BY id ASC LIMIT ?3",
        [thread.into(), after.into(), (limit + 1).into()],
    )?
    .map(decode_sized_entry)
    .collect::<Result<Vec<_>, _>>()?;

    let mut entries = Vec::with_capacity(fetched.len().min(limit));
    let mut bytes = 0usize;
    let mut has_newer = fetched.len() > limit;
    for (mut entry, stored_bytes) in fetched.into_iter().take(limit) {
        let size = preview(&mut entry, stored_bytes);
        let needed = size.saturating_add(usize::from(!entries.is_empty()));
        if !entries.is_empty() && bytes.saturating_add(needed) > budget {
            has_newer = true;
            break;
        }
        bytes = bytes.saturating_add(needed);
        entries.push(entry);
    }

    let older_cursor = if let Some(first) = entries.first()
        && has_visible_before(db, thread, first.id)?
    {
        Some(first.id)
    } else {
        None
    };
    let newer_cursor = if has_newer {
        entries.last().map(|entry| entry.id)
    } else {
        None
    };
    Ok(EntryPage {
        entries,
        before: older_cursor,
        after: newer_cursor,
    })
}

fn page_around(
    db: &Conn,
    thread: ThreadId,
    target: EntryId,
    limit: usize,
    budget: usize,
) -> Result<EntryPage, Error> {
    let turn_start = row(
        db,
        "SELECT id FROM entries
         WHERE thread = ?1 AND id <= ?2 AND json_extract(data, '$.kind') IN ('prompt', 'continued')
         ORDER BY id DESC LIMIT 1",
        [thread.into(), target.into()],
    )?
    .map(|r| r.id::<EntryId>(0))
    .transpose()?;
    let half_limit = (limit / 2).max(1);
    let half_budget = (budget / 2).max(limits::ENTRY_WIRE_BUDGET);

    let back_fetched = rows(
        db,
        "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
         WHERE thread = ?1 AND id < ?2 AND json_extract(data, '$.kind') != 'tool_update'
         ORDER BY id DESC LIMIT ?3",
        [thread.into(), target.into(), (half_limit + 1).into()],
    )?
    .map(decode_sized_entry)
    .collect::<Result<Vec<_>, _>>()?;

    let mut back = Vec::with_capacity(back_fetched.len().min(half_limit));
    let mut back_bytes = 0usize;
    let mut has_older = back_fetched.len() > half_limit;
    for (mut entry, stored_bytes) in back_fetched.into_iter().take(half_limit) {
        let size = preview(&mut entry, stored_bytes);
        let needed = size.saturating_add(usize::from(!back.is_empty()));
        let must_include_turn = turn_start.is_some_and(|start| entry.id >= start);
        if !back.is_empty() && !must_include_turn && back_bytes.saturating_add(needed) > half_budget
        {
            has_older = true;
            break;
        }
        if back_bytes.saturating_add(needed) > budget {
            has_older = true;
            break;
        }
        back_bytes = back_bytes.saturating_add(needed);
        back.push(entry);
    }
    if has_older
        && back.last().is_some_and(|oldest| !is_turn_start(oldest))
        && let Some(cut) = back.iter().rposition(is_turn_start)
    {
        back.truncate(cut + 1);
    }
    back.reverse();

    let fwd_limit = limit.saturating_sub(back.len()).max(1);
    let fwd_budget = budget.saturating_sub(back_bytes);
    let fwd_fetched = rows(
        db,
        "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
         WHERE thread = ?1 AND id >= ?2 AND json_extract(data, '$.kind') != 'tool_update'
         ORDER BY id ASC LIMIT ?3",
        [thread.into(), target.into(), (fwd_limit + 1).into()],
    )?
    .map(decode_sized_entry)
    .collect::<Result<Vec<_>, _>>()?;

    let mut forward = Vec::with_capacity(fwd_fetched.len().min(fwd_limit));
    let mut fwd_bytes = 0usize;
    let mut has_newer = fwd_fetched.len() > fwd_limit;
    for (mut entry, stored_bytes) in fwd_fetched.into_iter().take(fwd_limit) {
        let size = preview(&mut entry, stored_bytes);
        let needed = size.saturating_add(usize::from(!forward.is_empty() || !back.is_empty()));
        if !forward.is_empty() && fwd_bytes.saturating_add(needed) > fwd_budget {
            has_newer = true;
            break;
        }
        fwd_bytes = fwd_bytes.saturating_add(needed);
        forward.push(entry);
    }

    back.extend(forward);
    let older_cursor = if let Some(first) = back.first()
        && (has_older || has_visible_before(db, thread, first.id)?)
    {
        Some(first.id)
    } else {
        None
    };
    let newer_cursor = if let Some(last) = back.last()
        && (has_newer || has_visible_after(db, thread, last.id)?)
    {
        Some(last.id)
    } else {
        None
    };
    Ok(EntryPage {
        entries: back,
        before: older_cursor,
        after: newer_cursor,
    })
}

const fn is_turn_start(entry: &Entry) -> bool {
    matches!(entry.data, NewEntry::Prompt { .. } | NewEntry::Continued)
}

fn has_visible_before(db: &Conn, thread: ThreadId, id: EntryId) -> Result<bool, Error> {
    Ok(row(
        db,
        "SELECT 1 FROM entries
         WHERE thread = ?1 AND id < ?2 AND json_extract(data, '$.kind') != 'tool_update'
         LIMIT 1",
        [thread.into(), id.into()],
    )?
    .is_some())
}

fn has_visible_after(db: &Conn, thread: ThreadId, id: EntryId) -> Result<bool, Error> {
    Ok(row(
        db,
        "SELECT 1 FROM entries
         WHERE thread = ?1 AND id > ?2 AND json_extract(data, '$.kind') != 'tool_update'
         LIMIT 1",
        [thread.into(), id.into()],
    )?
    .is_some())
}

/// Reads incremental entries after `after`, stopping early when either `limit`
/// or `PAGE_WIRE_BYTES` is reached. Returns `(entries, has_more)`.
pub(in crate::workspace) fn entries_after(
    db: &Conn,
    thread: ThreadId,
    after: Option<EntryId>,
    limit: usize,
) -> Result<(Vec<Entry>, bool), Error> {
    const PAGE_BUDGET: usize = limits::PAGE_WIRE_BYTES;

    require(db, thread)?;
    let fetched = rows(
        db,
        "SELECT id, data, length(CAST(data AS BLOB)) FROM entries
         WHERE thread = ?1 AND id > coalesce(?2, 0) ORDER BY id LIMIT ?3",
        [thread.into(), after.into(), (limit + 1).into()],
    )?
    .map(decode_sized_entry)
    .collect::<Result<Vec<_>, _>>()?;
    let mut entries = Vec::with_capacity(fetched.len().min(limit));
    let mut bytes = 0usize;
    let mut has_more = fetched.len() > limit;
    for (mut entry, stored_bytes) in fetched.into_iter().take(limit) {
        let size = preview(&mut entry, stored_bytes);
        let needed = size.saturating_add(usize::from(!entries.is_empty()));
        if !entries.is_empty() && bytes.saturating_add(needed) > PAGE_BUDGET {
            has_more = true;
            break;
        }
        bytes = bytes.saturating_add(needed);
        entries.push(entry);
    }
    Ok((entries, has_more))
}

fn decode_raw_entry(mut row: Row) -> Result<Entry, Error> {
    Ok(Entry {
        id: row.id(0)?,
        full: None,
        data: from_json(&row.str(1)?)?,
    })
}

fn decode_sized_entry(mut row: Row) -> Result<(Entry, u64), Error> {
    let id = row.id(0)?;
    let data = from_json(&row.str(1)?)?;
    let stored_bytes = row.u64(2)?;
    Ok((
        Entry {
            id,
            full: None,
            data,
        },
        stored_bytes,
    ))
}

pub(in crate::workspace) fn search(
    db: &Conn,
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
    )?
    .map(|mut row| {
        Ok(SearchHit {
            thread: row.id(0)?,
            entry: row.id(1)?,
            snippet: row.str(2)?,
        })
    })
    .collect()
}
