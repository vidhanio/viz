//! Shared persistence helpers and aggregate-specific stores.

mod entry_wire;
mod fork;
mod lifecycle;
mod projects;
mod requests;
mod threads;
mod transcript;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, de::DeserializeOwned};

pub(super) use self::{
    entry_wire::{chunk, live_preview},
    fork::{copy_history, fork_prompt, has_entry, turns},
    lifecycle::{lifecycle, save_lifecycle, standings, touch},
    projects::{directory, path_text, preferences, project, project_at, projects, thread_worktree},
    requests::{
        access, chosen_access, insert_request, request, save_project_access, save_request,
        save_thread_access, withdraw_requests,
    },
    threads::{
        Binding, Handoff, NewThread, SavedSession, binding, create_thread, list_threads, metadata,
        require, save_metadata, state, updated,
    },
    transcript::{append, entries, entries_after, latest_entry_id, publish, search},
};
use crate::error::Error;

pub(super) fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    Ok(serde_json::to_string(value)?)
}

pub(super) fn from_json<T: DeserializeOwned>(value: &str) -> Result<T, Error> {
    serde_json::from_str(value).map_err(Error::corrupt)
}

pub(super) fn now() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
