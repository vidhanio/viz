//! Session commands for sessions no process has open, through a short-lived
//! harness process.

use std::{path::Path, sync::Arc, time::Duration};

use crate::{Client, ClientEvents, Error, HarnessConfig, error::Result};

/// Bounds the disposable process.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What to do with a saved session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Housekeeping {
    Delete,
    Archive,
    Unarchive,
}

/// Starts the harness in `cwd` and applies `command` to session `id`.
/// Returns whether the harness supports the command; one that does not is
/// left alone.
///
/// # Errors
/// When the harness fails to start, refuses the command, or times out.
pub async fn housekeep(
    config: &HarnessConfig,
    cwd: &Path,
    id: &str,
    command: Housekeeping,
) -> Result<bool> {
    let run = async {
        let agent =
            Client::start(config, cwd, Arc::new(|_, _| {}), ClientEvents::default()).await?;
        let capabilities = agent.capabilities();
        match command {
            Housekeeping::Delete if capabilities.delete => agent.delete_session(id).await.map(drop),
            Housekeeping::Archive if capabilities.archive => {
                agent.archive_session(id).await.map(drop)
            }
            Housekeeping::Unarchive if capabilities.archive => {
                agent.unarchive_session(id).await.map(drop)
            }
            Housekeeping::Delete | Housekeeping::Archive | Housekeeping::Unarchive => {
                return Ok(false);
            }
        }?;
        Ok(true)
    };
    tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| Error::Unavailable(format!("{} did not answer in time", config.command)))?
}
