//! Where the backend is: the local socket or a remote URL with its token.

use std::{fmt, path::PathBuf};

use viz_config::{Config, ResolveSecretError};

/// Where the backend is.
///
/// `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The local Unix socket. No token: socket permissions are the auth.
    Unix(PathBuf),
    /// A remote backend's `http://host:port` and its token.
    Remote {
        /// Base URL, without a trailing slash.
        url: String,
        /// Bearer token.
        token: String,
    },
}

impl Endpoint {
    /// `[client].url` with its token if set, else the local socket.
    ///
    /// # Errors
    ///
    /// Returns an error if a remote URL is set but no token resolves.
    pub fn from_config(config: &Config) -> Result<Self, ResolveEndpointError> {
        let Some(url) = &config.client.url else {
            return Ok(Self::Unix(config.backend_socket()));
        };
        let token = config
            .client
            .token
            .as_ref()
            .or(config.backend.token.as_ref())
            .ok_or(ResolveEndpointError::MissingToken)?
            .resolve()?;
        Ok(Self::Remote {
            url: url.trim_end_matches('/').to_owned(),
            token,
        })
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(path) => f.debug_tuple("Unix").field(path).finish(),
            Self::Remote { url, .. } => f
                .debug_struct("Remote")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(path) => write!(f, "unix:{}", path.display()),
            Self::Remote { url, .. } => f.write_str(url),
        }
    }
}

/// Why [`Endpoint::from_config`] failed.
#[derive(Debug, thiserror::Error)]
pub enum ResolveEndpointError {
    /// `[client].url` is set but neither `[client].token` nor
    /// `[backend].token` is.
    #[error("resolving secret: [client].url requires [client].token")]
    MissingToken,
    /// The token didn't resolve.
    #[error(transparent)]
    Token(#[from] ResolveSecretError),
}
