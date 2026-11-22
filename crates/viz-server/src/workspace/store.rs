//! Shared persistence helpers and aggregate-specific stores.

mod projects;
mod threads;
mod transcript;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Serialize, de::DeserializeOwned};

pub(super) use self::{
    projects::{directory, path_text, project, project_at, projects, thread_worktree},
    threads::{
        Binding, NewThread, SavedSession, binding, create_thread, list_threads, missing, require,
        state,
    },
    transcript::{append, entries, entries_after, publish, search},
};
use crate::wire::RpcError;

pub(super) fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, RpcError> {
    serde_json::to_string(value).map_err(RpcError::internal)
}

pub(super) fn from_json<T: DeserializeOwned>(value: &str) -> Result<T, RpcError> {
    serde_json::from_str(value).map_err(RpcError::internal)
}

pub(super) fn now() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}
