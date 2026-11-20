//! Validated backend addresses and their optional credentials.

use std::{fmt, net::IpAddr};

use tokio_tungstenite::tungstenite::http::Uri;
use viz_config::{Connection, DEFAULT_URL, ResolveSecretError};

/// An HTTP(S) origin and, when configured, a bearer token.
///
/// `Debug` never shows the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub(crate) url: String,
    pub(crate) token: Option<String>,
}

impl Endpoint {
    /// Resolves connection settings, using the loopback origin by default.
    ///
    /// # Errors
    ///
    /// Returns an error if the origin is invalid, uses HTTP off loopback, or
    /// a bearer token is required but cannot be resolved.
    pub fn from_connection(connection: &Connection) -> Result<Self, ResolveEndpointError> {
        let url = connection.url.as_deref().unwrap_or(DEFAULT_URL);
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
            .transpose()?;
        if !loopback && token.is_none() {
            return Err(ResolveEndpointError::MissingToken);
        }
        Ok(Self {
            url: format!("{scheme}://{authority}"),
            token,
        })
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// Why [`Endpoint::from_connection`] failed.
#[derive(Debug, thiserror::Error)]
pub enum ResolveEndpointError {
    /// The address is not a valid HTTP(S) origin.
    #[error("[backend].url must be an HTTP or HTTPS origin without a path, query, or credentials")]
    InvalidUrl,
    /// Only loopback origins may use plaintext HTTP.
    #[error("[backend].url must use HTTPS outside loopback")]
    RemoteHttp,
    /// The remote origin needs a bearer token.
    #[error("a non-loopback [backend].url requires [backend].token")]
    MissingToken,
    /// The configured token could not be resolved.
    #[error(transparent)]
    Token(#[from] ResolveSecretError),
}
