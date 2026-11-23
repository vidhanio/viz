//! Commands for sessions no process has open, through a short-lived harness
//! process: delete or archive one, list them, or load one's history.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    Client, ClientEvents, Error, HarnessConfig, ObservedUpdate, Output, SessionSetup, Transcript,
    error::Result,
    schema::{ContentBlock, ListSessionsRequest, SessionNotification, SessionUpdate},
};

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

/// A session the harness saved, as `session/list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedSession {
    pub id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    /// The harness's timestamp, as it wrote it.
    pub updated_at: Option<String>,
}

/// One page of the sessions the harness saved for `cwd`, and the cursor of
/// the next. `None` when the harness cannot list sessions.
///
/// # Errors
/// When the harness fails to start, refuses the request, or times out.
pub async fn saved_sessions(
    config: &HarnessConfig,
    cwd: &Path,
    cursor: Option<String>,
) -> Result<Option<(Vec<SavedSession>, Option<String>)>> {
    let run = async {
        let agent =
            Client::start(config, cwd, Arc::new(|_, _| {}), ClientEvents::default()).await?;
        if !agent.capabilities().list {
            return Ok(None);
        }
        let response = agent
            .list_sessions(
                ListSessionsRequest::new()
                    .cwd(cwd.to_owned())
                    .cursor(cursor),
            )
            .await?;
        let sessions = response
            .sessions
            .into_iter()
            .map(|session| SavedSession {
                id: session.session_id.0.to_string(),
                cwd: session.cwd,
                title: session.title,
                updated_at: session.updated_at,
            })
            .collect();
        Ok(Some((sessions, response.next_cursor)))
    };
    tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| Error::Unavailable(format!("{} did not answer in time", config.command)))?
}

/// One turn of a loaded session: what the user said, and the agent's output
/// folded as a live turn would be.
#[derive(Debug)]
pub struct LoadedTurn {
    pub prompt: String,
    pub entries: Vec<Output>,
    pub answer: String,
}

/// Loads session `id` in a disposable process and folds the history it
/// replays into turns, keyed from `turn_key` with the turn's index. `None`
/// when the harness cannot load sessions.
///
/// # Errors
/// When the harness fails to start, refuses to load, or times out.
///
/// # Panics
/// If the replay buffer's lock was poisoned by a panicking update sink.
pub async fn load_history(
    config: &HarnessConfig,
    cwd: &Path,
    id: &str,
    turn_key: &str,
) -> Result<Option<Vec<LoadedTurn>>> {
    let replayed: Arc<Mutex<Vec<ObservedUpdate>>> = Arc::default();
    let sink = {
        let replayed = Arc::clone(&replayed);
        let id = id.to_owned();
        Arc::new(move |notification: SessionNotification, observed_at: i64| {
            if notification.session_id.0.as_ref() == id {
                replayed
                    .lock()
                    .expect("load replay lock poisoned")
                    .push(ObservedUpdate {
                        update: notification.update,
                        observed_at,
                    });
            }
        })
    };
    let run = async {
        let agent = Client::start(config, cwd, sink, ClientEvents::default()).await?;
        if !agent.capabilities().load {
            return Ok::<_, Error>(None);
        }
        agent.load_session(id, &SessionSetup::new(cwd)).await?;
        Ok(Some(()))
    };
    let loaded = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| Error::Unavailable(format!("{} did not answer in time", config.command)))??;
    if loaded.is_none() {
        return Ok(None);
    }
    let updates = std::mem::take(&mut *replayed.lock().expect("load replay lock poisoned"));
    Ok(Some(fold_history(updates, turn_key)))
}

/// Splits a replay into turns at each user message after agent output.
fn fold_history(updates: Vec<ObservedUpdate>, turn_key: &str) -> Vec<LoadedTurn> {
    let mut turns = Vec::new();
    let mut prompt = String::new();
    let mut agent = Vec::new();
    for update in updates {
        if let SessionUpdate::UserMessageChunk(chunk) = &update.update {
            if !agent.is_empty() {
                finish_turn(&mut prompt, &mut agent, &mut turns, turn_key);
            }
            if let ContentBlock::Text(text) = &chunk.content {
                prompt.push_str(&text.text);
            }
        } else {
            agent.push(update);
        }
    }
    finish_turn(&mut prompt, &mut agent, &mut turns, turn_key);
    turns
}

fn finish_turn(
    prompt: &mut String,
    agent: &mut Vec<ObservedUpdate>,
    turns: &mut Vec<LoadedTurn>,
    turn_key: &str,
) {
    if prompt.is_empty() && agent.is_empty() {
        return;
    }
    let mut transcript = Transcript::new(format!("{turn_key}:{}", turns.len()), Arc::default());
    let mut entries = transcript.apply(std::mem::take(agent));
    entries.extend(transcript.finish());
    turns.push(LoadedTurn {
        prompt: std::mem::take(prompt),
        entries,
        answer: transcript.answer().to_owned(),
    });
}
