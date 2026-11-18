//! Errors talking to the backend.

use crate::protocol::PROTOCOL_VERSION;

/// Errors talking to the backend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Couldn't reach the backend.
    #[error("connecting to {endpoint}: {source}")]
    Connect {
        /// The endpoint tried.
        endpoint: String,
        /// The underlying error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The backend returned a JSON-RPC error.
    #[error("{method}: backend error {code}: {message}")]
    Rpc {
        /// The method called.
        method: &'static str,
        /// JSON-RPC error code.
        code: i64,
        /// Error message.
        message: String,
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
    #[error("connection to the backend closed")]
    Closed,
    /// The backend speaks another protocol version.
    #[error("backend speaks protocol {backend}, this client speaks {PROTOCOL_VERSION}")]
    Protocol {
        /// The backend's version.
        backend: u32,
    },
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
