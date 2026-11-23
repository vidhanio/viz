//! Connection settings common to all viz clients.

use serde::Deserialize;

use crate::Secret;

/// The default local server origin.
pub const DEFAULT_URL: &str = "http://127.0.0.1:7420";

/// The `[client.server]` connection table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Base HTTP(S) origin, defaulting to [`DEFAULT_URL`].
    pub url: Option<String>,
    /// Bearer token required for a non-loopback address.
    pub token: Option<Secret>,
}

/// The shared `[client]` table, read by every program that talks to a server.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Client {
    /// The server every client connects to.
    pub server: Connection,
}
