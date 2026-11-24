//! Upgrade the workspace database at startup.
//!
//! One transaction holds the write lock while the old database is backed up
//! and every pending migration runs, so a failed upgrade leaves the database
//! as it was. Foreign keys are not enforced meanwhile: dropping a table to
//! rebuild it would otherwise delete the rows referring to it. They are
//! checked before the commit instead.

use std::{
    collections::{HashMap, HashSet},
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
    include_str!("../../../migrations/0014_performance_and_hydration.sql"),
    include_str!("../../../migrations/0015_prompts.sql"),
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
        if version == 15 {
            migrate_prompts(&tx)?;
        }
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

/// Converts legacy prompt rows in the same transaction as the schema update.
fn migrate_prompts(tx: &Connection) -> anyhow::Result<()> {
    #[derive(serde::Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum LegacyContext {
        File {
            path: String,
        },
        Link {
            uri: String,
            name: String,
            title: Option<String>,
        },
    }
    fn convert(
        text: &str,
        attachments: Vec<viz_protocol::PromptAttachment>,
        context: Vec<LegacyContext>,
    ) -> viz_protocol::Prompt {
        let mut prompt = migrate_attachment_links(text, attachments);
        for target in context {
            let (label, target) = match target {
                LegacyContext::File { path } => (
                    format!("@{path}"),
                    viz_protocol::PromptRef::File {
                        path,
                        directory: false,
                    },
                ),
                LegacyContext::Link { uri, name, title } => (
                    name.clone(),
                    viz_protocol::PromptRef::Link { uri, name, title },
                ),
            };
            prompt.append_reference(&label, target);
        }
        prompt
    }
    #[derive(serde::Deserialize)]
    struct LegacyPrompt {
        text: String,
        #[serde(default)]
        attachments: Vec<viz_protocol::PromptAttachment>,
        #[serde(default)]
        context: Vec<LegacyContext>,
        origin: Option<viz_protocol::Origin>,
    }

    let mut select = tx.prepare("SELECT id, data FROM entries WHERE data ->> 'kind' = 'prompt'")?;
    let mut entries = select.query([])?;
    while let Some(entry) = entries.next()? {
        let id = entry.get::<_, i64>(0)?;
        let data: serde_json::Value = serde_json::from_str(&entry.get::<_, String>(1)?)?;
        let old: LegacyPrompt = serde_json::from_value(data["data"].clone())?;
        let prompt = convert(&old.text, old.attachments, old.context);
        let data = serde_json::to_string(&viz_protocol::EntryData::Prompt {
            prompt,
            origin: old.origin,
        })?;
        tx.execute(
            "UPDATE entries SET data = ?2 WHERE id = ?1",
            params![id, data],
        )?;
    }
    let mut select = tx.prepare("SELECT id, text, attachments, context FROM runs")?;
    let mut runs = select.query([])?;
    while let Some(run) = runs.next()? {
        let id = run.get::<_, i64>(0)?;
        let text = run.get::<_, String>(1)?;
        let attachments = run.get::<_, Option<String>>(2)?;
        let context = run.get::<_, Option<String>>(3)?;
        let prompt = convert(
            &text,
            serde_json::from_str::<Vec<_>>(attachments.as_deref().unwrap_or("[]"))?,
            serde_json::from_str::<Vec<_>>(context.as_deref().unwrap_or("[]"))?,
        );
        tx.execute(
            "UPDATE runs SET text = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(&prompt)?],
        )?;
    }
    Ok(())
}

/// Resolves old generated attachment labels using CommonMark's reference-link
/// rules. An attachment without an inline occurrence still receives a link.
fn migrate_attachment_links(
    text: &str,
    attachments: Vec<viz_protocol::PromptAttachment>,
) -> viz_protocol::Prompt {
    use pulldown_cmark::{BrokenLink, Event, LinkType, Options, Parser, Tag};
    use viz_protocol::{Prompt, PromptRef};

    if attachments.is_empty() {
        return Prompt::new(text);
    }
    let mut labels = HashMap::with_capacity(attachments.len());
    let mut names = Vec::with_capacity(attachments.len());
    let mut prompt = Prompt::default();
    let mut images = 0;
    let mut files = 0;
    for (index, attachment) in attachments.into_iter().enumerate() {
        let key = if attachment.mime.starts_with("image/") {
            images += 1;
            format!("img-{images}")
        } else {
            files += 1;
            format!("file-{files}")
        };
        labels.insert(key, index);
        names.push(attachment.name.clone());
        prompt.refs.push(PromptRef::Attachment {
            id: attachment.id,
            name: attachment.name,
            mime: attachment.mime,
        });
    }
    let mut broken = |link: BrokenLink<'_>| {
        let index = labels.get(&link.reference.to_ascii_lowercase())?;
        Some((index.to_string().into(), "".into()))
    };
    let parser = Parser::new_with_broken_link_callback(text, Options::empty(), Some(&mut broken));
    let mut found = HashSet::new();
    let mut end = 0;
    for (event, range) in parser.into_offset_iter() {
        if let Event::Start(
            Tag::Link {
                link_type: LinkType::ReferenceUnknown,
                dest_url,
                ..
            }
            | Tag::Image {
                link_type: LinkType::ReferenceUnknown,
                dest_url,
                ..
            },
        ) = event
            && range.start >= end
            && let Ok(index) = dest_url.parse::<usize>()
        {
            prompt.text.push_str(&text[end..range.start]);
            prompt
                .text
                .push_str(&Prompt::reference_link(&names[index], index));
            found.insert(index);
            end = range.end;
        }
    }
    prompt.text.push_str(&text[end..]);
    let mut unreferenced = String::new();
    for (index, name) in names.iter().enumerate() {
        if !found.contains(&index) {
            unreferenced.push_str(&Prompt::reference_link(name, index));
            unreferenced.push_str("\n\n");
        }
    }
    prompt.text.insert_str(0, &unreferenced);
    prompt
}
