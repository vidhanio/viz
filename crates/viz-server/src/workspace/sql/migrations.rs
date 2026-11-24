//! Upgrade the workspace database at startup.
//!
//! One transaction holds the write lock while the old database is backed up
//! and every pending migration runs, so a failed upgrade leaves the database
//! as it was. Foreign keys are not enforced meanwhile: dropping a table to
//! rebuild it would otherwise delete the rows referring to it. They are
//! checked before the commit instead.

use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, bail, ensure};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, backup::Backup, params,
};

use crate::workspace::blobs;

const MIGRATIONS: &[&str] = &[
    include_str!("../../../migrations/0001_baseline.sql"),
    include_str!("../../../migrations/0002_dismissed_tasks.sql"),
    include_str!("../../../migrations/0003_lifecycle.sql"),
    include_str!("../../../migrations/0004_forks.sql"),
    include_str!("../../../migrations/0005_project_settings.sql"),
    include_str!("../../../migrations/0006_checkpoints.sql"),
    include_str!("../../../migrations/0007_checkpoint_releases.sql"),
    include_str!("../../../migrations/0008_rich_transcript.sql"),
    include_str!("../../../migrations/0009_requests.sql"),
    include_str!("../../../migrations/0010_harness_status.sql"),
    include_str!("../../../migrations/0011_prompts.sql"),
    include_str!("../../../migrations/0012_tool_index.sql"),
    include_str!("../../../migrations/0013_blobs.sql"),
];
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn open(path: &Path) -> anyhow::Result<Connection> {
    let mut db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    db.pragma_update(None, "foreign_keys", false)?;
    migrate(&mut db, path)?;
    db.pragma_update(None, "foreign_keys", true)?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(db)
}

/// Opens a read-only WAL connection for concurrent queries outside the writer
/// thread.
pub(super) fn open_reader(path: &Path) -> anyhow::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = Connection::open_with_flags(path, flags)
        .with_context(|| format!("opening read-only {}", path.display()))?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    db.pragma_update(None, "query_only", true)?;
    Ok(db)
}

fn migrate(db: &mut Connection, path: &Path) -> anyhow::Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = tx.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))?;
    let current = usize::try_from(current).context("invalid workspace schema version")?;
    ensure!(
        current <= MIGRATIONS.len(),
        "workspace schema version {current} exceeds supported version {}",
        MIGRATIONS.len()
    );
    if current == MIGRATIONS.len() {
        return Ok(());
    }
    let populated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
        [],
        |row| row.get(0),
    )?;
    if populated {
        backup(path, current)?;
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let version = index + 1;
        tx.execute_batch(sql)
            .with_context(|| format!("applying workspace migration {version:04}"))?;
        if populated && version == 1 {
            ensure_baseline(&tx)?;
        }
        if version == 13 {
            migrate_blobs(&tx, path)?;
        }
        tx.pragma_update(None, "user_version", i32::try_from(version)?)?;
    }
    let violation = tx
        .query_row("PRAGMA foreign_key_check", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    if let Some(table) = violation {
        bail!("migrating the workspace left dangling references in {table}");
    }
    tx.commit()?;
    Ok(())
}

/// Databases from before migrations carry no version, and the baseline leaves
/// their tables alone. Refuse one whose tables lack columns instead of failing
/// on it later.
fn ensure_baseline(db: &Connection) -> anyhow::Result<()> {
    const COLUMNS: &str = "SELECT t.name, c.name FROM sqlite_schema t, pragma_table_info(t.name) c
        WHERE t.type = 'table'";
    let columns = |db: &Connection| {
        db.prepare(COLUMNS)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<HashSet<(String, String)>>>()
    };
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(MIGRATIONS[0])?;
    let found = columns(db)?;
    if let Some((table, column)) = columns(&expected)?.difference(&found).next() {
        bail!("the workspace predates migrations and its {table} table has no {column} column");
    }
    Ok(())
}

/// Copy the database as it was at `version`. Reading through SQLite includes
/// pages still in the write-ahead log.
fn backup(path: &Path, version: usize) -> anyhow::Result<()> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let backup = path.with_extension(format!("v{version:04}.sqlite3"));
    let mut destination = Connection::open(&backup)?;
    Backup::new(&source, &mut destination)?
        .run_to_completion(128, Duration::from_millis(10), None)
        .with_context(|| format!("backing up workspace to {}", backup.display()))?;
    Ok(())
}

/// Moves inline SQLite attachment BLOBs into the content-addressed on-disk
/// blob store (`blobs/sha256/<ab>/<hex>`) and replaces `attachments` with
/// `blobs` + `thread_attachments`.
fn migrate_blobs(tx: &Connection, db_path: &Path) -> anyhow::Result<()> {
    let has_legacy: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'attachments')",
        [],
        |row| row.get(0),
    )?;
    if !has_legacy {
        return Ok(());
    }
    let workspace_dir = db_path
        .parent()
        .context("workspace database path has no parent directory")?;
    let blobs_dir = workspace_dir.join("blobs");
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX);
    let mut select = tx.prepare("SELECT thread, id, name, mime, data FROM attachments")?;
    let mut insert_blob = tx.prepare(
        "INSERT INTO blobs (id, size, mime, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT DO NOTHING",
    )?;
    let mut insert_ref = tx.prepare(
        "INSERT INTO thread_attachments (thread, id, blob, name, mime)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT DO NOTHING",
    )?;
    let mut rows = select.query([])?;
    while let Some(row) = rows.next()? {
        let thread: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let name: String = row.get(2)?;
        let mime: String = row.get(3)?;
        let data: Vec<u8> = row.get(4)?;
        let size = i64::try_from(data.len()).context("attachment exceeds SQLite integer range")?;
        let blob_id = blobs::write_blob_sync(&blobs_dir, &data)?;
        insert_blob.execute(params![blob_id, size, mime, now])?;
        insert_ref.execute(params![thread, id, blob_id, name, mime])?;
    }
    drop(rows);
    drop(select);
    drop(insert_blob);
    drop(insert_ref);
    tx.execute_batch("DROP TABLE attachments;")?;
    Ok(())
}
