//! Errors talking to the server.

use crate::protocol::PROTOCOL_VERSION;

/// Errors talking to the server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Couldn't reach the server.
    #[error("connecting to {endpoint}: {source}")]
    Connect {
        /// The endpoint tried.
        endpoint: String,
        /// The underlying error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The server returned a JSON-RPC error.
    #[error("{method}: server error {code}: {message}")]
    Rpc {
        /// The method called.
        method: &'static str,
        /// JSON-RPC error code.
        code: i64,
        /// Error message.
        message: String,
        /// Machine-readable Git operation failure, when applicable.
        git_failure: Option<viz_protocol::GitFailure>,
    },
    /// A request's params couldn't be encoded as JSON.
    #[error("{method}: encoding params: {source}")]
    Encode {
        /// The method called.
        method: &'static str,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// A response didn't match the expected shape.
    #[error("{method}: unexpected response: {source}")]
    Decode {
        /// The method called.
        method: &'static str,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// The connection closed.
    #[error("connection to the server closed")]
    Closed,
    /// The server speaks another protocol version.
    #[error("server speaks protocol {server}, this client speaks {PROTOCOL_VERSION}")]
    Protocol {
        /// The server's version.
        server: u32,
    },
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
