//! Where the backend is: the local socket or a remote URL with its token.

use std::{fmt, path::PathBuf};

use viz_config::{Connection, Paths, ResolveSecretError};

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
    /// Uses the remote URL when set, otherwise the configured or default
    /// socket.
    ///
    /// # Errors
    ///
    /// Returns an error if a remote URL is set but no token resolves.
    pub fn from_connection(
        connection: &Connection,
        paths: &Paths,
    ) -> Result<Self, ResolveEndpointError> {
        let Some(url) = &connection.url else {
            return Ok(Self::Unix(
                connection
                    .socket
                    .clone()
                    .unwrap_or_else(|| paths.default_socket()),
            ));
        };
        let token = connection
            .token
            .as_ref()
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

/// Why [`Endpoint::from_connection`] failed.
#[derive(Debug, thiserror::Error)]
pub enum ResolveEndpointError {
    /// `[backend].url` is set but `[backend].token` is not.
    #[error("resolving secret: [backend].url requires [backend].token")]
    MissingToken,
    /// The token didn't resolve.
    #[error(transparent)]
    Token(#[from] ResolveSecretError),
}
