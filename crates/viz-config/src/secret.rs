//! Secrets given inline, in a file, or in an environment variable.

use std::{env, fmt, fs, io, path::PathBuf};

use serde::Deserialize;

/// A secret given inline, read from a file, or read from an environment
/// variable.
///
/// ```toml
/// token = "inline"                         # discouraged outside throwaway setups
/// token = { file = "/run/secrets/viz" } # sops-nix, agenix, systemd credentials
/// token = { env = "VIZ_TOKEN" }
/// ```
///
/// `Debug` never shows an inline value.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Secret {
    /// The secret value itself.
    Inline(String),
    /// A file whose contents (trailing newline trimmed) are the secret.
    File {
        /// Path to the file. `$CREDENTIALS_DIRECTORY`-relative paths are not
        /// expanded.
        file: PathBuf,
    },
    /// An environment variable holding the secret.
    Env {
        /// Name of the variable.
        env: String,
    },
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(_) => f.write_str("Secret(<inline>)"),
            Self::File { file } => f.debug_struct("Secret").field("file", file).finish(),
            Self::Env { env } => f.debug_struct("Secret").field("env", env).finish(),
        }
    }
}

impl Secret {
    /// Resolves the secret's value.
    ///
    /// # Errors
    ///
    /// Returns a [`ResolveSecretError`] if the file or variable can't be read,
    /// or the value is empty.
    pub fn resolve(&self) -> Result<String, ResolveSecretError> {
        let value = match self {
            Self::Inline(value) => value.clone(),
            Self::File { file } => {
                let mut value =
                    fs::read_to_string(file).map_err(|source| ResolveSecretError::File {
                        path: file.clone(),
                        source,
                    })?;
                if value.ends_with('\n') {
                    value.pop();
                    if value.ends_with('\r') {
                        value.pop();
                    }
                }
                value
            }
            Self::Env { env } => env::var(env).map_err(|source| ResolveSecretError::Env {
                name: env.clone(),
                source,
            })?,
        };
        if value.is_empty() {
            return Err(ResolveSecretError::Empty {
                secret: self.clone(),
            });
        }
        Ok(value)
    }
}

/// Why a [`Secret`] didn't resolve.
#[derive(Debug, thiserror::Error)]
pub enum ResolveSecretError {
    /// The secret's file could not be read.
    #[error("resolving secret: {path}: {source}")]
    File {
        /// The file.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
    /// The secret's environment variable is unset or not Unicode.
    #[error("resolving secret: ${name}: {source}")]
    Env {
        /// The variable's name.
        name: String,
        /// Why it couldn't be read.
        source: env::VarError,
    },
    /// The secret resolved to an empty value.
    #[error("resolving secret: {secret:?} is empty")]
    Empty {
        /// The secret; being empty, it reveals nothing.
        secret: Secret,
    },
}
