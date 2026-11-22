//! Startup owns the migration lock until the backup and all pending DDL are
//! durable.

use std::{path::Path, time::Duration};

use anyhow::{Context as _, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, backup::Backup};

const MIGRATIONS: &[&str] = &[include_str!("../../../migrations/0001_baseline.sql")];
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn open(path: &Path) -> anyhow::Result<Connection> {
    let mut db = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    db.pragma_update(None, "foreign_keys", true)?;
    migrate(&mut db, path)?;
    db.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
    db.pragma_update(None, "journal_mode", "WAL")?;
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
        tx.pragma_update(None, "user_version", i32::try_from(version)?)?;
    }
    tx.commit()?;
    Ok(())
}

/// Read through SQLite so committed WAL pages are included in the snapshot.
fn backup(path: &Path, version: usize) -> anyhow::Result<()> {
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let backup = path.with_extension(format!("pre-v{version:04}.sqlite3"));
    let mut destination = Connection::open(&backup)?;
    Backup::new(&source, &mut destination)?
        .run_to_completion(128, Duration::from_millis(10), None)
        .with_context(|| format!("backing up workspace to {}", backup.display()))?;
    Ok(())
}
