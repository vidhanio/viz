//! Validated server endpoints: private local Unix sockets or authenticated
//! HTTP(S) origins.

use std::{fmt, net::IpAddr, path::PathBuf};

use tokio_tungstenite::tungstenite::http::Uri;
use viz_config::{Connection, ResolveSecretError, default_socket, validate_socket_path};

/// A validated server endpoint: either a local Unix domain socket or an
/// authenticated HTTP(S) origin.
///
/// `Debug` never shows the bearer token.
#[derive(Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Local Unix domain socket authenticated by filesystem permissions.
    Unix { path: PathBuf },
    /// Authenticated HTTP(S) TCP origin.
    Http { url: String, token: String },
}

impl Endpoint {
    /// Resolves connection settings, defaulting to
    /// [`viz_config::default_socket`] when neither `socket` nor `url` is set.
    ///
    /// # Errors
    ///
    /// Returns an error if both `socket` and `url` are set, the socket path or
    /// HTTP(S) origin is invalid, a non-loopback origin uses plaintext HTTP, or
    /// a TCP `url` has no resolvable bearer token.
    pub fn from_connection(connection: &Connection) -> Result<Self, ResolveEndpointError> {
        if connection.socket.is_some() && connection.url.is_some() {
            return Err(ResolveEndpointError::BothSocketAndUrl);
        }
        if let Some(url) = connection.url.as_deref() {
            let uri: Uri = url.parse().map_err(|_| ResolveEndpointError::InvalidUrl)?;
            let scheme = uri.scheme_str().ok_or(ResolveEndpointError::InvalidUrl)?;
            let authority = uri.authority().ok_or(ResolveEndpointError::InvalidUrl)?;
            if !matches!(scheme, "http" | "https")
                || authority.as_str().contains('@')
                || url.contains('#')
                || !matches!(uri.path(), "" | "/")
                || uri.query().is_some()
            {
                return Err(ResolveEndpointError::InvalidUrl);
            }
            let host = authority.host();
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback());
            if !loopback && scheme == "http" {
                return Err(ResolveEndpointError::RemoteHttp);
            }
            let token = connection
                .token
                .as_ref()
                .map(viz_config::Secret::resolve)
                .transpose()?
                .filter(|token| !token.trim().is_empty())
                .ok_or(ResolveEndpointError::MissingToken)?;
            return Ok(Self::Http {
                url: format!("{scheme}://{authority}"),
                token,
            });
        }
        let path = match &connection.socket {
            Some(path) => {
                validate_socket_path(path)?;
                path.clone()
            }
            None => default_socket()?,
        };
        Ok(Self::Unix { path })
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix { path } => f
                .debug_struct("Endpoint::Unix")
                .field("path", path)
                .finish(),
            Self::Http { url, .. } => f
                .debug_struct("Endpoint::Http")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix { path } => write!(f, "unix://{}", path.display()),
            Self::Http { url, .. } => f.write_str(url),
        }
    }
}

/// Why [`Endpoint::from_connection`] failed.
#[derive(Debug, thiserror::Error)]
pub enum ResolveEndpointError {
    /// Both `socket` and `url` were configured simultaneously.
    #[error("[client.server] cannot set both socket and url")]
    BothSocketAndUrl,
    /// The address is not a valid HTTP(S) origin.
    #[error(
        "[client.server].url must be an HTTP or HTTPS origin without a path, query, or credentials"
    )]
    InvalidUrl,
    /// Only loopback origins may use plaintext HTTP.
    #[error("[client.server].url must use HTTPS outside loopback")]
    RemoteHttp,
    /// Every TCP origin requires a bearer token.
    #[error("[client.server].url requires a non-empty [client.server].token")]
    MissingToken,
    /// The configured token could not be resolved.
    #[error(transparent)]
    Token(#[from] ResolveSecretError),
    /// The default or configured Unix socket path is invalid.
    #[error(transparent)]
    Socket(#[from] viz_config::Error),
}
