//! The sections every Rust component reads: `[backend]` and `[client]`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::Secret;

/// `[backend]`: where the backend listens. Clients on the same machine use
/// these too.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackendSection {
    /// Unix socket path. Default:
    /// [`Paths::backend_socket`](crate::Paths::backend_socket).
    pub socket: Option<PathBuf>,
    /// Optional TCP `host:port` listener for remote clients (tailnet/LAN).
    /// Requires `token`.
    pub listen: Option<String>,
    /// Bearer token remote clients must present.
    pub token: Option<Secret>,
}

/// `[client]`: how Rust clients (connectors, desktop, CLI) reach the backend.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ClientSection {
    /// `http://host:port` of a remote backend. Unset: the local socket from
    /// `[backend]`.
    pub url: Option<String>,
    /// Token for `url`. Unset: `[backend].token`.
    pub token: Option<Secret>,
}
