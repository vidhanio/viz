//! Project registry and harness catalog operations exposed by workspace RPCs.

use std::path::Path;

use futures_util::{StreamExt as _, stream::FuturesUnordered};

use super::{
    Workspace, runtime,
    sql::{exec, rows},
    store,
};
use crate::{
    acp,
    wire::{Harness, Project, RpcError},
};

impl Workspace {
    pub(super) async fn create_project(
        &self,
        path: &str,
        name: Option<String>,
    ) -> Result<Project, RpcError> {
        if path.is_empty() || name.as_deref() == Some("") {
            return Err(RpcError::invalid("project path and name must not be empty"));
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
        let created_at = store::now();
        let count = exec(&mut db, "INSERT INTO projects (path, name, created_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            [path.into(), name.as_str().into(), created_at.into()]).await?;
        drop(db);
        if count == 0 {
            return Err(RpcError::invalid(format!("project already exists: {path}")));
        }
        self.changed(None);
        Ok(store::project_at(path.to_owned(), name, created_at))
    }

    pub(super) async fn remove_project(&self, project: &str) -> Result<(), RpcError> {
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
            return Err(RpcError::not_found(format!("no project {project}")));
        }
        tx.commit().await?;
        drop(db);
        self.changed(None);
        for thread in threads {
            self.changed(Some(thread.u64(0)?));
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
    ) -> Result<Vec<crate::wire::ConfigOption>, RpcError> {
        let (_, config) = self.harness(Some(name))?;
        let stored = {
            let mut db = self.db.lock().await;
            runtime::catalog(&mut db, name).await?
        };
        let (options, updated) =
            acp::catalog(config, self.default_cwd(), stored.as_ref(), model).await?;
        if let Some(updated) = updated {
            runtime::save_catalog(&mut *self.db.lock().await, name, &updated).await?;
        }
        Ok(options)
    }
}
