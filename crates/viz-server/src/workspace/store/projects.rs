//! Project records and presentation of checked-out Git metadata.

use std::{
    env,
    path::{Path, PathBuf},
};

use viz_core::projects::Preferences;
use viz_protocol::ProjectSettings;

use super::super::{
    sql::{Connection, row, rows},
    store::from_json,
    worktree,
};
use crate::{
    error::Error,
    wire::{Project, ThreadWorktree},
};

pub(in crate::workspace) async fn directory(path: &Path) -> Result<PathBuf, Error> {
    let path = if let Ok(rest) = path.strip_prefix("~") {
        PathBuf::from(env::var_os("HOME").ok_or_else(|| Error::invalid("HOME is not set"))?)
            .join(rest)
    } else {
        path.to_path_buf()
    };
    let canonical = tokio::fs::canonicalize(&path)
        .await
        .map_err(|e| Error::invalid(format!("cannot read directory {}: {e}", path.display())))?;
    let metadata = tokio::fs::metadata(&canonical).await?;
    if !metadata.is_dir() {
        return Err(Error::invalid(format!(
            "not a directory: {}",
            path.display()
        )));
    }
    Ok(canonical)
}

pub(in crate::workspace) fn path_text(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| Error::invalid("working directory is not UTF-8"))
}

/// Persisted project row fetched under the SQLite lock before async Git
/// enrichment runs outside the lock.
#[derive(Debug)]
pub(in crate::workspace) struct ProjectRow {
    pub(in crate::workspace) path: String,
    pub(in crate::workspace) name: String,
    pub(in crate::workspace) created_at: i64,
    pub(in crate::workspace) settings: ProjectSettings,
}

pub(in crate::workspace) async fn require_project(
    db: &mut Connection,
    path: &str,
) -> Result<String, Error> {
    let Some(mut found) = row(
        db,
        "SELECT path FROM projects WHERE path = ?1",
        [path.into()],
    )
    .await?
    else {
        return Err(Error::NoProject(path.to_owned()));
    };
    found.str(0)
}

pub(in crate::workspace) async fn project_row(
    db: &mut Connection,
    path: &str,
) -> Result<ProjectRow, Error> {
    let (name, created_at, prefs) = preferences(db, path).await?;
    Ok(ProjectRow {
        path: path.to_owned(),
        name,
        created_at,
        settings: prefs.settings,
    })
}

pub(in crate::workspace) async fn preferences(
    db: &mut Connection,
    path: &str,
) -> Result<(String, i64, Preferences), Error> {
    let mut row = row(
        db,
        "SELECT name, created_at, settings FROM projects WHERE path = ?1",
        [path.into()],
    )
    .await?
    .ok_or_else(|| Error::NoProject(path.to_owned()))?;
    Ok((row.str(0)?, row.i64(1)?, from_json(&row.str(2)?)?))
}

/// A project with its checkout's Git state (must run outside any SQLite lock).
pub(in crate::workspace) async fn project_at(
    path: String,
    name: String,
    created_at: i64,
    settings: ProjectSettings,
) -> Project {
    let checkout = worktree::head(Path::new(&path)).await;
    Project {
        settings,
        git: checkout.is_some(),
        branch: checkout.flatten(),
        path,
        name,
        created_at,
    }
}

/// A thread's worktree, with its branch read from disk (must run outside any
/// SQLite lock).
pub(in crate::workspace) async fn thread_worktree(path: Option<String>) -> Option<ThreadWorktree> {
    let path = path?;
    Some(ThreadWorktree {
        branch: worktree::head(Path::new(&path)).await.flatten(),
        path,
    })
}

pub(in crate::workspace) async fn project_rows(
    db: &mut Connection,
) -> Result<Vec<ProjectRow>, Error> {
    let records = rows(
        db,
        "SELECT path, name, created_at, settings FROM projects ORDER BY name, path",
        [],
    )
    .await?;
    let mut out = Vec::new();
    for mut record in records {
        let prefs: Preferences = from_json(&record.str(3)?)?;
        out.push(ProjectRow {
            path: record.str(0)?,
            name: record.str(1)?,
            created_at: record.i64(2)?,
            settings: prefs.settings,
        });
    }
    Ok(out)
}
