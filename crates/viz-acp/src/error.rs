use std::{io, path::PathBuf};

use thiserror::Error;

/// ACP transport, lifecycle, and turn errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("starting ACP harness {command}")]
    Spawn {
        command: String,
        #[source]
        source: io::Error,
    },
    #[error("ACP harness {0} was not started with piped stdio")]
    MissingPipe(String),
    #[error("ACP harness closed: {0}")]
    Closed(String),
    #[error("ACP {method}: {source}")]
    Protocol {
        method: String,
        #[source]
        source: Box<agent_client_protocol::Error>,
    },
    #[error("serializing JSON-RPC")]
    Json(#[from] serde_json::Error),
    #[error("unsupported ACP protocol version {0}")]
    Version(u16),
    #[error("the harness does not support {0}")]
    Unsupported(&'static str),
    #[error("invalid ACP path {0}")]
    Path(PathBuf),
    #[error("{operation}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("resolving harness environment variable {0}")]
    Environment(String),
    #[error("{0} is not available")]
    Unavailable(String),
    #[error("the harness did not apply {0}")]
    Selection(String),
    #[error("invalid value for option {0}")]
    InvalidOption(String),
    #[error("probing ACP options timed out")]
    ProbeTimeout,
    #[error("the turn was cancelled before it started")]
    Cancelled,
    /// A prompt failed after reaching the harness.
    #[error(transparent)]
    Turn(#[from] crate::TurnFailure),
    #[error("the ACP harness did not end the cancelled turn")]
    CancelTimeout,
    #[error("publishing ACP updates")]
    Publish(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("MCP bridge HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("the {0} tool's input schema is not an object")]
    ToolSchema(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    #[must_use]
    pub fn protocol(method: &str, source: agent_client_protocol::Error) -> Self {
        Self::Protocol {
            method: method.to_owned(),
            source: Box::new(source),
        }
    }
}
