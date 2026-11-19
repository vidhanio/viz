//! Connection settings common to all viz clients.

use std::path::PathBuf;

use serde::Deserialize;

use crate::Secret;

/// The client's `[backend]` connection table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Local socket, defaulting to [`crate::Paths::default_socket`].
    pub socket: Option<PathBuf>,
    /// Remote base URL, taking precedence over the local socket.
    pub url: Option<String>,
    /// Bearer token required for a remote URL.
    pub token: Option<Secret>,
}
