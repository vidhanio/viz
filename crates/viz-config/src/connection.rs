//! Connection settings common to all viz clients.

use serde::Deserialize;

use crate::Secret;

/// The default local server origin.
pub const DEFAULT_URL: &str = "http://127.0.0.1:7420";

/// The client's `[server]` connection table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Base HTTP(S) origin, defaulting to [`DEFAULT_URL`].
    pub url: Option<String>,
    /// Bearer token required for a non-loopback address.
    pub token: Option<Secret>,
}
