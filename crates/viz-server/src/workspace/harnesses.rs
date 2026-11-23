//! The harness catalogue: what each configured harness offers and whether it
//! can run threads.
//!
//! The list answers from what the last check stored. Checks run in the
//! background, one at a time per harness, when a harness was never checked or
//! its result is old; a turn that fails because the user is signed out or out
//! of usage records that at once. Each change is announced, so clients list
//! harnesses again.

use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};

use viz_core::requests;
use viz_protocol::{HarnessStatus, ThreadId};

use super::{
    Workspace, sessions,
    sql::{Connection, exec, row},
    store,
};
use crate::{
    acp,
    error::Error,
    wire::{ConfigOption, Harness},
};

/// How long a check stands, in milliseconds: a working harness is checked
/// hourly, one with a problem every five minutes.
const READY_FOR: i64 = 60 * 60 * 1000;
const PROBLEM_FOR: i64 = 5 * 60 * 1000;

/// Harnesses with a check under way.
#[derive(Default)]
pub(super) struct Checks(Mutex<HashSet<String>>);

impl Workspace {
    /// Every configured harness as last checked. Starts checks for the stale.
    pub(super) async fn list_harnesses(self: &Arc<Self>) -> Result<Vec<Harness>, Error> {
        let now = store::now();
        let mut harnesses = Vec::with_capacity(self.harnesses.len());
        let mut db = self.db.lock().await;
        for (name, config) in &self.harnesses {
            let catalog = sessions::catalog(&mut db, name).await?;
            let (status, checked_at) = status(&mut db, name)
                .await?
                .map_or((HarnessStatus::Unchecked, None), |(status, at)| {
                    (status, Some(at))
                });
            let stale_after = if status == HarnessStatus::Ready {
                READY_FOR
            } else {
                PROBLEM_FOR
            };
            if checked_at.is_none_or(|at| now - at > stale_after) {
                self.check(name);
            }
            let options = catalog.map_or_else(Vec::new, |catalog| catalog.options(config, None));
            harnesses.push(Harness {
                id: name.clone(),
                name: self.harness_name(name).to_owned(),
                default: self.default_harness.as_deref() == Some(name),
                icon: icon(&config.command).to_owned(),
                status,
                checked_at,
                access: requests::access_modes(&options),
                options,
            });
        }
        drop(db);
        harnesses.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(harnesses)
    }

    /// Checks a harness in the background unless a check is under way.
    fn check(self: &Arc<Self>, name: &str) {
        if !self
            .checks
            .0
            .lock()
            .expect("harness checks poisoned")
            .insert(name.to_owned())
        {
            return;
        }
        let workspace = Arc::clone(self);
        let name = name.to_owned();
        self.jobs.spawn(async move {
            if let Err(error) = workspace.run_check(&name).await {
                tracing::warn!(harness = %name, %error, "checking an ACP harness");
            }
            workspace
                .checks
                .0
                .lock()
                .expect("harness checks poisoned")
                .remove(&name);
        });
    }

    async fn run_check(&self, name: &str) -> Result<(), Error> {
        let (_, config) = self.harness(Some(name))?;
        let stored = sessions::catalog(&mut *self.db.lock().await, name).await?;
        let checked = acp::refresh(config, self.default_cwd(), stored.as_ref()).await;
        let mut db = self.db.lock().await;
        let status = match checked {
            Ok(catalog) => {
                sessions::save_catalog(&mut db, name, &catalog).await?;
                HarnessStatus::Ready
            }
            Err(error) => acp::availability(&error).unwrap_or_else(|| HarnessStatus::Failed {
                message: error.to_string(),
            }),
        };
        save_status(&mut db, name, &status).await?;
        drop(db);
        self.harnesses_changed();
        Ok(())
    }

    /// Records what a thread's failed turn says about its harness.
    pub(super) async fn harness_failed(&self, thread: ThreadId, error: &Error) {
        let Error::Harness(error) = error else {
            return;
        };
        let name = match store::binding(&mut *self.db.lock().await, thread).await {
            Ok(binding) => binding.harness,
            Err(_) => return,
        };
        let name = name.as_str();
        let Some(
            status @ (HarnessStatus::NotInstalled { .. }
            | HarnessStatus::SignedOut { .. }
            | HarnessStatus::LimitReached { .. }),
        ) = acp::availability(error)
        else {
            return;
        };
        if let Err(error) = save_status(&mut *self.db.lock().await, name, &status).await {
            tracing::warn!(harness = %name, %error, "recording an ACP harness status");
        }
        self.harnesses_changed();
    }

    /// The options a new thread with `model` starts with, learning them from
    /// the harness when the catalogue lacks them.
    pub(super) async fn harness_options(
        &self,
        name: &str,
        model: Option<&str>,
    ) -> Result<Vec<ConfigOption>, Error> {
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

/// The mark for a harness command, by its file name.
fn icon(command: &str) -> &'static str {
    let name = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    match name {
        name if name.starts_with("claude") => "claude",
        name if name.starts_with("codex") => "codex",
        "agy" | "antigravity" | "gemini" | "gemini-cli" => "gemini",
        "omp" => "omp",
        name if name.starts_with("pi") => "pi",
        _ => "acp",
    }
}

async fn status(db: &mut Connection, name: &str) -> Result<Option<(HarnessStatus, i64)>, Error> {
    row(
        db,
        "SELECT status, checked_at FROM harness_status WHERE name = ?1",
        [name.into()],
    )
    .await?
    .map(|mut row| Ok((store::from_json(&row.str(0)?)?, row.i64(1)?)))
    .transpose()
}

async fn save_status(db: &mut Connection, name: &str, status: &HarnessStatus) -> Result<(), Error> {
    exec(
        db,
        "INSERT INTO harness_status (name, status, checked_at) VALUES (?1, ?2, ?3)
        ON CONFLICT (name) DO UPDATE SET status = excluded.status, checked_at = excluded.checked_at",
        [name.into(), store::json(status)?.into(), store::now().into()],
    )
    .await?;
    Ok(())
}
