//! The `[server]` table of the shared viz config file. Runner credentials stay
//! with their runners.

use std::{
    collections::BTreeMap,
    env, fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use axum::http::HeaderValue;
use serde::Deserialize;
use url::Url;
use viz_config::{Component, Paths, Secret, default_socket, validate_socket_path};

#[derive(Debug)]
pub(crate) struct Config {
    pub(crate) environment: String,
    pub(crate) cwd: PathBuf,
    pub(crate) state_dir: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) listen: Option<SocketAddr>,
    pub(crate) token: Option<Arc<str>>,
    pub(crate) default_harness: Option<String>,
    pub(crate) harnesses: BTreeMap<String, HarnessConfig>,
    pub(crate) environments: BTreeMap<String, PeerConfig>,
}

pub(crate) use viz_acp::HarnessConfig;

pub(crate) struct PeerConfig {
    pub(crate) url: Url,
    pub(crate) token: String,
    pub(crate) accept: Option<Arc<str>>,
}

impl fmt::Debug for PeerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerConfig")
            .field("url", &self.url)
            .field("accept", &self.accept.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Server {
    environment: Option<String>,
    cwd: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    listen: Option<SocketAddr>,
    token: Option<Secret>,
    default_harness: Option<String>,
    harnesses: BTreeMap<String, HarnessConfig>,
    environments: BTreeMap<String, PeerFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerFile {
    url: String,
    token: Secret,
    accept: Option<Secret>,
}

impl Config {
    pub(crate) fn load(explicit: Option<&Path>) -> Result<Self> {
        let paths = Paths::from_env()?;
        let server: Server = Component::Server
            .load(explicit)
            .context("invalid server configuration")?;
        let environment = server.environment.unwrap_or_else(|| String::from("local"));
        ensure!(
            valid_name(&environment),
            "invalid environment ID {environment:?}"
        );
        let socket = match server.socket {
            Some(path) => {
                validate_socket_path(&path)?;
                path
            }
            None => default_socket()?,
        };
        let token = server
            .token
            .as_ref()
            .map(resolve_bearer)
            .transpose()
            .context("invalid server.token")?;
        for (id, harness) in &server.harnesses {
            ensure!(valid_name(id), "invalid harness ID {id:?}");
            ensure!(
                harness
                    .name
                    .as_ref()
                    .is_none_or(|name| !name.trim().is_empty()),
                "harness {id:?} has an empty display name"
            );
            ensure!(
                !harness.command.is_empty(),
                "harness {id:?} has no executable"
            );
        }
        let default_harness = match server.default_harness {
            Some(name) => {
                ensure!(
                    server.harnesses.contains_key(&name),
                    "unconfigured default harness {name:?}"
                );
                Some(name)
            }
            None if server.harnesses.len() == 1 => server.harnesses.keys().next().cloned(),
            None => None,
        };
        let (environments, has_peer_accept) = load_peers(
            server.environments,
            &environment,
            server.listen.is_some(),
            token.as_ref(),
        )?;
        if server.listen.is_some() {
            ensure!(
                token.is_some() || has_peer_accept,
                "server.listen requires server.token or at least one peer accept credential"
            );
        }
        let cwd = working_directory(server.cwd)?;
        Ok(Self {
            environment,
            cwd,
            state_dir: server.state_dir.unwrap_or(paths.state_dir),
            socket,
            listen: server.listen,
            token,
            default_harness,
            harnesses: server.harnesses,
            environments,
        })
    }
}

fn load_peers(
    raw: BTreeMap<String, PeerFile>,
    local_env: &str,
    has_tcp: bool,
    client_token: Option<&Arc<str>>,
) -> Result<(BTreeMap<String, PeerConfig>, bool)> {
    let mut environments = BTreeMap::new();
    let mut inbound: Vec<Arc<str>> = client_token.cloned().into_iter().collect();
    let mut has_peer_accept = false;
    for (name, peer) in raw {
        ensure!(
            valid_name(&name) && name != local_env,
            "invalid peer environment ID {name:?}"
        );
        let url = Url::parse(&peer.url).with_context(|| format!("invalid peer URL for {name}"))?;
        ensure!(
            url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "peer {name} must be a base URL without credentials or query parameters"
        );
        let outbound = peer.token.resolve()?;
        let accept = peer
            .accept
            .as_ref()
            .map(resolve_bearer)
            .transpose()
            .with_context(|| format!("invalid accept token for peer {name}"))?;
        if let Some(accept_token) = &accept {
            ensure!(has_tcp, "peer {name}.accept requires server.listen");
            ensure!(
                !inbound.iter().any(|existing| existing == accept_token),
                "peer {name}.accept must be distinct from server.token and other peers"
            );
            inbound.push(Arc::clone(accept_token));
            has_peer_accept = true;
        }
        environments.insert(
            name,
            PeerConfig {
                url,
                token: outbound,
                accept,
            },
        );
    }
    Ok((environments, has_peer_accept))
}

fn resolve_bearer(secret: &Secret) -> Result<Arc<str>> {
    let value = secret.resolve()?;
    ensure!(!value.trim().is_empty(), "bearer token must not be empty");
    HeaderValue::from_str(&format!("Bearer {value}"))
        .context("bearer token contains invalid HTTP header characters")?;
    Ok(Arc::from(value))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn working_directory(configured: Option<PathBuf>) -> Result<PathBuf> {
    let cwd = configured.map_or_else(
        || {
            env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME is unset")
        },
        Ok,
    )?;
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("resolving working directory {}", cwd.display()))?;
    ensure!(cwd.is_dir(), "server working directory is not a directory");
    Ok(cwd)
}
