//! Connection settings common to all viz clients.

use std::path::PathBuf;

use serde::Deserialize;

use crate::Secret;

/// The `[client.server]` connection table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Local Unix domain socket path. Defaults to [`crate::default_socket`]
    /// when neither `socket` nor `url` is set.
    pub socket: Option<PathBuf>,
    /// Base HTTP(S) origin for a TCP server (mutually exclusive with `socket`;
    /// always requires `token`).
    pub url: Option<String>,
    /// Bearer token required whenever `url` is set.
    pub token: Option<Secret>,
}

/// The shared `[client]` table, read by every program that talks to a server.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Client {
    /// The server every client connects to.
    pub server: Connection,
}
