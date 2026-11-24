//! Copy a thread's history into its fork.

use std::collections::HashMap;

use viz_core::fork::{Cut, Turn};
use viz_protocol::{EntryId, Prompt, ThreadId};

use super::{
    super::sql::{Conn, exec, row, rows},
    from_json, json,
};
use crate::{error::Error, wire::NewEntry};

/// The started runs of a thread, and its last entry.
pub(in crate::workspace) fn turns(
    db: &Conn,
    thread: ThreadId,
) -> Result<(Vec<Turn>, Option<EntryId>), Error> {
    let turns = rows(
        db,
        "SELECT id, prompt_entry, end_entry FROM runs
         WHERE thread = ?1 AND prompt_entry IS NOT NULL AND (end_entry IS NOT NULL OR status = 'running')
         ORDER BY prompt_entry",
        [thread.into()],
    )?
    .map(|row| {
        Ok(Turn {
            run: row.id(0)?,
            start: row.id(1)?,
            end: row.opt_id(2)?,
        })
    })
    .collect::<Result<Vec<_>, Error>>()?;
    let last = row(
        db,
        "SELECT max(id) FROM entries WHERE thread = ?1",
        [thread.into()],
    )?
    .map_or(Ok(None), |row| row.opt_id(0))?;
    Ok((turns, last))
}

/// Whether `entry` belongs to `thread`.
pub(in crate::workspace) fn has_entry(
    db: &Conn,
    thread: ThreadId,
    entry: EntryId,
) -> Result<bool, Error> {
    Ok(row(
        db,
        "SELECT 1 FROM entries WHERE thread = ?1 AND id = ?2",
        [thread.into(), entry.into()],
    )?
    .is_some())
}

/// The user's prompt at an entry.
pub(in crate::workspace) fn fork_prompt(
    db: &Conn,
    thread: ThreadId,
    entry: EntryId,
) -> Result<Option<Prompt>, Error> {
    let Some(mut row) = row(
        db,
        "SELECT data FROM entries WHERE thread = ?1 AND id = ?2",
        [thread.into(), entry.into()],
    )?
    else {
        return Ok(None);
    };
    Ok(match from_json(&row.str(0)?)? {
        NewEntry::Prompt { prompt, .. } => Some(prompt),
        _ => None,
    })
}

/// Copy the entries, runs and attachments of a cut, and return the last entry
/// copied. Search finds the originals only.
pub(in crate::workspace) fn copy_history(
    db: &Conn,
    source: ThreadId,
    fork: ThreadId,
    cut: &Cut,
) -> Result<Option<EntryId>, Error> {
    let Some(through) = cut.through else {
        return Ok(None);
    };
    let mut last = None;
    let mut copies = HashMap::new();
    for mut entry in rows(
        db,
        "SELECT id, data FROM entries WHERE thread = ?1 AND id <= ?2 ORDER BY id",
        [source.into(), through.into()],
    )? {
        let mut data = entry.str(1)?;
        if let NewEntry::ToolUpdate { entry, record } = from_json(&data)? {
            let entry = copies.get(&entry).copied().unwrap_or(entry);
            data = json(&NewEntry::ToolUpdate { entry, record })?;
        }
        let copy = row(
            db,
            "INSERT INTO entries (thread, data) VALUES (?1, ?2) RETURNING id",
            [fork.into(), data.into()],
        )?
        .expect("entry INSERT RETURNING yielded one row")
        .id::<EntryId>(0)?;
        let original = entry.id(0)?;
        copies.insert(original, copy);
        last = Some(original);
    }
    for &run in &cut.runs {
        let Some(bounds) = row(
            db,
            "SELECT prompt_entry, end_entry FROM runs WHERE id = ?1",
            [run.into()],
        )?
        else {
            continue;
        };
        let target = |entry: Option<EntryId>| entry.and_then(|entry| copies.get(&entry).copied());
        exec(
            db,
            "INSERT INTO runs (thread, kind, status, position, prompt, origin, prompt_entry, end_entry, answer, error, usage)
             SELECT ?1, kind, status, 0, prompt, origin, ?3, ?4, answer, error, usage FROM runs WHERE id = ?2",
            [
                fork.into(),
                run.into(),
                target(bounds.opt_id(0)?).into(),
                target(bounds.opt_id(1)?).into(),
            ],
        )?;
    }
    exec(
        db,
        "INSERT INTO thread_attachments (thread, id, blob, name, mime)
         SELECT ?1, id, blob, name, mime FROM thread_attachments WHERE thread = ?2 AND id IN
            (SELECT a.value ->> 'id' FROM runs AS r, json_each(r.prompt, '$.refs') AS a
             WHERE r.thread = ?1 AND a.value ->> 'type' = 'attachment')
         ON CONFLICT DO NOTHING",
        [fork.into(), source.into()],
    )?;
    Ok(last)
}
