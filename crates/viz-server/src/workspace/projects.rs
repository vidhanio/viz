//! Project registry operations exposed by workspace RPCs.

use std::path::Path;

use viz_core::projects;
use viz_protocol::{ProjectSettings, ProjectSettingsPatch, Setting, ThreadId};

use super::{
    Workspace,
    sql::{exec, rows},
    store,
};
use crate::{error::Error, wire::Project};

impl Workspace {
    pub(super) async fn create_project(
        &self,
        path: &str,
        name: Option<String>,
    ) -> Result<Project, Error> {
        if path.is_empty() || name.as_deref() == Some("") {
            return Err(Error::invalid("project path and name must not be empty"));
        }
        let canonical = store::directory(Path::new(path)).await?;
        let path = store::path_text(&canonical)?.to_owned();
        let name = name.unwrap_or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&path)
                .to_owned()
        });
        let path_for_tx = path.clone();
        let (name, created_at, settings) = self
            .db
            .write_tx(move |tx| {
                let requested_at = store::now();
                let count = exec(
                    tx,
                    "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                    [path_for_tx.as_str().into(), name.as_str().into(), requested_at.into()],
                )?;
                if count == 0 {
                    let (name, created_at, current) = store::preferences(tx, &path_for_tx)?;
                    let prefs = projects::edit(
                        current,
                        ProjectSettingsPatch {
                            archived: Some(false),
                            ..ProjectSettingsPatch::default()
                        },
                    )?;
                    exec(
                        tx,
                        "UPDATE projects SET settings = ?2 WHERE path = ?1",
                        [path_for_tx.as_str().into(), store::json(&prefs)?.into()],
                    )?;
                    Ok((name, created_at, prefs.settings))
                } else {
                    Ok((name, requested_at, ProjectSettings::default()))
                }
            })
            .await?;
        self.project_changed(path.clone());
        Ok(store::project_at(path, name, created_at, settings).await)
    }

    pub(super) async fn update_project(
        &self,
        principal_key: &str,
        path: &str,
        name: Option<String>,
        mut changes: ProjectSettingsPatch,
    ) -> Result<(), Error> {
        if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
            return Err(projects::Rejection::EmptyName.into());
        }
        if let Some(Setting::Set(harness)) = &changes.default_harness {
            self.harness(Some(harness))?;
        }
        if let Some(dirs) = &mut changes.additional_directories {
            for dir in dirs.iter_mut() {
                let candidate = Path::new(dir);
                let absolute = if candidate.starts_with("~") {
                    candidate.to_owned()
                } else {
                    Path::new(path).join(candidate)
                };
                store::path_text(&store::directory(&absolute).await?)?.clone_into(dir);
            }
            dirs.sort_unstable();
            dirs.dedup();
        }
        self.validate_uploaded_icon(principal_key, path, changes.icon.as_ref())
            .await?;
        let icon_changed = changes.icon.is_some();
        let path_owned = path.to_owned();
        let path_for_tx = path_owned.clone();
        self.db
            .write_tx(move |tx| {
                let (current_name, _, current) = store::preferences(tx, &path_for_tx)?;
                let updated = projects::edit(current, changes)?;
                if icon_changed {
                    exec(
                        tx,
                        "UPDATE projects
                         SET name = ?2, settings = ?3,
                             icon_blob = NULL, icon_mime = NULL,
                             icon_fetched_at = NULL, icon_mode = NULL
                         WHERE path = ?1",
                        [
                            path_for_tx.as_str().into(),
                            name.unwrap_or(current_name).into(),
                            store::json(&updated)?.into(),
                        ],
                    )?;
                } else {
                    exec(
                        tx,
                        "UPDATE projects SET name = ?2, settings = ?3 WHERE path = ?1",
                        [
                            path_for_tx.as_str().into(),
                            name.unwrap_or(current_name).into(),
                            store::json(&updated)?.into(),
                        ],
                    )?;
                }
                Ok(())
            })
            .await?;
        self.project_changed(path_owned);
        Ok(())
    }

    pub(super) async fn remove_project(&self, project: &str) -> Result<(), Error> {
        let project_owned = project.to_owned();
        let project_for_tx = project_owned.clone();
        let threads: Vec<ThreadId> = self
            .db
            .write_tx(move |tx| {
                let threads: Vec<ThreadId> = rows(
                    tx,
                    "SELECT id FROM threads WHERE project = ?1",
                    [project_for_tx.as_str().into()],
                )?
                .map(|row| row.id(0))
                .collect::<Result<_, _>>()?;
                let count = exec(
                    tx,
                    "DELETE FROM projects WHERE path = ?1",
                    [project_for_tx.as_str().into()],
                )?;
                if count == 0 {
                    return Err(Error::NoProject(project_for_tx));
                }
                Ok(threads)
            })
            .await?;
        self.project_removed(project_owned);
        for thread in threads {
            self.relist(thread);
        }
        Ok(())
    }
}
