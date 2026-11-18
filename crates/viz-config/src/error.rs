//! Errors from locating, reading and deserializing configuration.

use std::{io, path::PathBuf};

/// Errors from locating, reading or deserializing configuration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Neither `$HOME` nor the relevant XDG variable is set.
    #[error("cannot locate {var}: set $HOME or ${var}")]
    NoHome {
        /// The XDG variable that was needed, e.g. `XDG_STATE_HOME`.
        var: &'static str,
    },
    /// A config layer could not be read.
    #[error("reading {path}: {source}")]
    Read {
        /// The file or directory being read.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// A config layer is not valid TOML.
    #[error("parsing {path}: {source}")]
    Parse {
        /// The file being parsed.
        path: PathBuf,
        /// The underlying TOML error.
        source: Box<toml::de::Error>,
    },
    /// The merged document does not match the requested section's schema.
    #[error("invalid [{section}]: {source}")]
    Section {
        /// The dotted section name, e.g. `connectors.discord`.
        section: String,
        /// The underlying TOML error.
        source: Box<toml::de::Error>,
    },
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
