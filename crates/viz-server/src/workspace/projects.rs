//! Project registry and harness catalog operations exposed by workspace RPCs.

use std::path::Path;

use futures_util::{StreamExt as _, stream::FuturesUnordered};
use viz_core::projects;
use viz_protocol::{ProjectSettings, ProjectSettingsPatch, Setting};

use super::{
    Workspace, sessions,
    sql::{exec, rows},
    store,
};
use crate::{
    acp,
    error::Error,
    wire::{Harness, Project},
};

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
        let path = store::path_text(&canonical)?;
        let name = name.unwrap_or_else(|| {
            canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_owned()
        });
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let requested_at = store::now();
        let count = exec(&mut tx, "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            [path.into(), name.as_str().into(), requested_at.into()]).await?;
        let (name, created_at, settings) = if count == 0 {
            let (name, created_at, current) = store::preferences(&mut tx, path).await?;
            let prefs = projects::edit(
                current,
                ProjectSettingsPatch {
                    archived: Some(false),
                    ..ProjectSettingsPatch::default()
                },
            )?;
            exec(
                &mut tx,
                "UPDATE projects SET settings = ?2 WHERE path = ?1",
                [path.into(), store::json(&prefs)?.into()],
            )
            .await?;
            (name, created_at, prefs.settings)
        } else {
            (name, requested_at, ProjectSettings::default())
        };
        tx.commit().await?;
        drop(db);
        self.changed(None);
        Ok(store::project_at(path.to_owned(), name, created_at, settings).await)
    }

    pub(super) async fn update_project(
        &self,
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
        super::icons::validate_upload(changes.icon.as_ref())?;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let (current_name, _, current) = store::preferences(&mut tx, path).await?;
        let updated = projects::edit(current, changes)?;
        exec(
            &mut tx,
            "UPDATE projects SET name = ?2, settings = ?3 WHERE path = ?1",
            [
                path.into(),
                name.unwrap_or(current_name).into(),
                store::json(&updated)?.into(),
            ],
        )
        .await?;
        tx.commit().await?;
        drop(db);
        self.changed(None);
        Ok(())
    }

    pub(super) async fn remove_project(&self, project: &str) -> Result<(), Error> {
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let threads = rows(
            &mut tx,
            "SELECT id FROM threads WHERE project = ?1",
            [project.into()],
        )
        .await?;
        let count = exec(
            &mut tx,
            "DELETE FROM projects WHERE path = ?1",
            [project.into()],
        )
        .await?;
        if count == 0 {
            return Err(Error::NoProject(project.to_owned()));
        }
        tx.commit().await?;
        drop(db);
        self.changed(None);
        for thread in threads {
            self.changed(Some(thread.id(0)?));
        }
        Ok(())
    }

    pub(super) async fn list_harnesses(&self) -> Vec<Harness> {
        let pending = self
            .harnesses
            .keys()
            .map(|name| self.harness_summary(name))
            .collect::<FuturesUnordered<_>>();
        let mut harnesses: Vec<_> = pending.collect().await;
        harnesses.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        harnesses
    }

    async fn harness_summary(&self, name: &str) -> Harness {
        let options = self
            .harness_options(name, None)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(harness = %name, %error, "ACP catalog unavailable");
                Vec::new()
            });
        Harness {
            id: name.to_owned(),
            name: self.harness_name(name).to_owned(),
            default: self.default_harness.as_deref() == Some(name),
            options,
        }
    }

    pub(super) async fn harness_options(
        &self,
        name: &str,
        model: Option<&str>,
    ) -> Result<Vec<crate::wire::ConfigOption>, Error> {
        let (_, config) = self.harness(Some(name))?;
        let stored = {
            let mut db = self.db.lock().await;
            sessions::catalog(&mut db, name).await?
        };
        let (options, updated) =
            acp::catalog(config, self.default_cwd(), stored.as_ref(), model).await?;
        if let Some(updated) = updated {
            sessions::save_catalog(&mut *self.db.lock().await, name, &updated).await?;
        }
        Ok(options)
    }
}
