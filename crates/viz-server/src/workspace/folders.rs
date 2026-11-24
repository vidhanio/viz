//! Browse folders on the server where projects and harnesses run.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use viz_protocol::{
    BrowseFolders, CreateFolder, FolderCreated, FolderEntries, FolderEntry, FolderIssue,
    InitRepository,
};

use super::{Workspace, sql::rows, store};
use crate::error::Error;

impl Workspace {
    pub(super) async fn browse_folders(&self, args: BrowseFolders) -> Result<FolderEntries, Error> {
        let projects = self
            .db
            .read(|conn| {
                rows(conn, "SELECT path FROM projects", [])?
                    .map(|mut row| row.str(0))
                    .collect::<Result<HashSet<_>, _>>()
            })
            .await?;
        let home = home_dir();
        let base = if let Some(project) = &args.project {
            if !projects.contains(project) {
                return Err(Error::NoProject(project.clone()));
            }
            Path::new(project)
        } else {
            self.default_cwd()
        };
        let raw = args.path.as_deref().unwrap_or_default();
        let requested = expand_path(base, Path::new(raw));
        let (directory, filter) = match tokio::fs::metadata(&requested).await {
            Ok(metadata) if metadata.is_dir() => (requested, None),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !raw.ends_with('/') => {
                let filter = requested
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned);
                let parent = requested.parent().map(Path::to_owned).unwrap_or(requested);
                (parent, filter)
            }
            _ => (requested, None),
        };
        let canonical = tokio::fs::canonicalize(&directory)
            .await
            .unwrap_or(directory);
        let path = store::path_text(&canonical)?.to_owned();
        let mut response = FolderEntries {
            project: projects.contains(&path),
            path,
            parent: canonical.parent().and_then(Path::to_str).map(str::to_owned),
            home: home.and_then(|path| path.into_os_string().into_string().ok()),
            git: false,
            completion: None,
            entries: Vec::new(),
            issue: None,
        };
        let mut entries = match tokio::fs::read_dir(&canonical).await {
            Ok(entries) => entries,
            Err(error) => {
                response.issue = Some(issue(&error));
                return Ok(response);
            }
        };
        response.git = self.is_git(&canonical).await;
        let prefix = filter.as_deref().unwrap_or_default();
        while let Some(entry) = match entries.next_entry().await {
            Ok(entry) => entry,
            Err(error) => {
                response.issue = Some(issue(&error));
                None
            }
        } {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if (!args.show_hidden && name.starts_with('.'))
                || !name.starts_with(prefix)
                || name == ".git"
            {
                continue;
            }
            let path = entry.path();
            let error = match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_dir() => None,
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    Some(FolderIssue::PermissionDenied)
                }
                _ => continue,
            };
            let path = tokio::fs::canonicalize(&path).await.unwrap_or(path);
            let text = store::path_text(&path)?.to_owned();
            response.entries.push(FolderEntry {
                project: projects.contains(&text),
                git: error.is_none() && self.is_git(&path).await,
                path: text,
                name,
                issue: error,
            });
        }
        response
            .entries
            .sort_unstable_by(|a, b| a.name.cmp(&b.name));
        if filter.is_some() {
            response.completion = common_completion(&canonical, prefix, &response.entries);
        }
        Ok(response)
    }

    pub(super) async fn create_folder(&self, args: CreateFolder) -> Result<FolderCreated, Error> {
        if args.path.trim().is_empty() {
            return Err(Error::invalid("folder path must not be empty"));
        }
        let path = expand_path(self.default_cwd(), Path::new(&args.path));
        tokio::fs::create_dir(&path).await?;
        folder_created(&store::directory(&path).await?)
    }

    pub(super) async fn init_repository(
        &self,
        args: InitRepository,
    ) -> Result<FolderCreated, Error> {
        let path =
            store::directory(&expand_path(self.default_cwd(), Path::new(&args.path))).await?;
        if let Ok(repo) = viz_git::Repository::discover(&path, &self.closing).await
            && (repo.identity().checkout.as_ref() == Some(&path)
                || repo.identity().common_dir == path)
        {
            return Err(Error::invalid("directory is already a Git repository"));
        }
        viz_git::Repository::init(&path, &self.closing).await?;
        folder_created(&path)
    }

    async fn is_git(&self, path: &Path) -> bool {
        viz_git::Repository::discover(path, &self.closing)
            .await
            .is_ok_and(|repo| repo.identity().checkout.is_some())
    }
}

fn folder_created(path: &Path) -> Result<FolderCreated, Error> {
    Ok(FolderCreated {
        path: store::path_text(path)?.to_owned(),
    })
}

pub(super) fn expand_path(base: &Path, path: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix("~")
        && let Some(home) = home_dir()
    {
        return home.join(relative);
    }
    base.join(path)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn issue(error: &io::Error) -> FolderIssue {
    match error.kind() {
        io::ErrorKind::PermissionDenied => FolderIssue::PermissionDenied,
        io::ErrorKind::NotFound => FolderIssue::NotFound,
        io::ErrorKind::NotADirectory => FolderIssue::NotDirectory,
        _ => FolderIssue::Unavailable,
    }
}

fn common_completion(directory: &Path, typed: &str, entries: &[FolderEntry]) -> Option<String> {
    let first = &entries.first()?.name;
    if entries.len() == 1 {
        return Some(format!("{}/", directory.join(first).to_str()?));
    }
    let last = &entries.last()?.name;
    let count = first
        .chars()
        .zip(last.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let prefix: String = first.chars().take(count).collect();
    (prefix.len() > typed.len())
        .then(|| directory.join(prefix).to_str().map(str::to_owned))
        .flatten()
}
