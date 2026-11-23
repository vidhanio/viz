//! Project records and presentation of checked-out Git metadata.

use std::{
    env,
    path::{Path, PathBuf},
};

use super::super::{
    sql::{Connection, row, rows},
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

pub(in crate::workspace) async fn project(
    db: &mut Connection,
    path: &str,
) -> Result<Project, Error> {
    let mut row = row(
        db,
        "SELECT name, created_at FROM projects WHERE path = ?1",
        [path.into()],
    )
    .await?
    .ok_or_else(|| Error::NoProject(path.to_owned()))?;
    Ok(project_at(path.to_owned(), row.str(0)?, row.i64(1)?))
}

/// A project with its checkout's Git state.
pub(in crate::workspace) fn project_at(path: String, name: String, created_at: i64) -> Project {
    let checkout = worktree::head(Path::new(&path));
    Project {
        git: checkout.is_some(),
        branch: checkout.and_then(|checkout| checkout.branch),
        path,
        name,
        created_at,
    }
}

/// A thread's worktree, with its branch read from disk.
pub(in crate::workspace) fn thread_worktree(path: String) -> ThreadWorktree {
    ThreadWorktree {
        branch: worktree::head(Path::new(&path)).and_then(|checkout| checkout.branch),
        path,
    }
}

pub(in crate::workspace) async fn projects(db: &mut Connection) -> Result<Vec<Project>, Error> {
    rows(
        db,
        "SELECT path, name, created_at FROM projects ORDER BY name, path",
        [],
    )
    .await?
    .map(|mut row| Ok(project_at(row.str(0)?, row.str(1)?, row.i64(2)?)))
    .collect()
}
