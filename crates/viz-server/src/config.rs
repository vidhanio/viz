//! Layered server TOML. Client files and runner credentials stay separate.

use std::{
    collections::BTreeMap,
    env, fmt, fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use url::Url;
use viz_config::{Paths, Secret};

#[derive(Debug)]
pub(crate) struct Config {
    pub(crate) environment: String,
    pub(crate) cwd: PathBuf,
    pub(crate) state_dir: PathBuf,
    pub(crate) worktree_dir: Option<PathBuf>,
    pub(crate) worktree_branch_prefix: String,
    pub(crate) listen: SocketAddr,
    pub(crate) token: Option<Secret>,
    pub(crate) default_harness: Option<String>,
    pub(crate) harnesses: BTreeMap<String, HarnessConfig>,
    pub(crate) environments: BTreeMap<String, PeerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HarnessConfig {
    pub(crate) name: Option<String>,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, Secret>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}

pub(crate) struct PeerConfig {
    pub(crate) url: Url,
    pub(crate) token: String,
}

impl fmt::Debug for PeerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerConfig")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    server: Server,
    harnesses: BTreeMap<String, HarnessConfig>,
    environments: BTreeMap<String, PeerFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Server {
    environment: Option<String>,
    cwd: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    /// Parent directory for generated checkout paths. Defaults to the
    /// environment state directory's worktrees subdirectory.
    worktree_dir: Option<PathBuf>,
    /// Prefix for branch names generated from thread titles. Defaults to
    /// "viz/".
    worktree_branch_prefix: Option<String>,
    listen: Option<SocketAddr>,
    token: Option<Secret>,
    default_harness: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerFile {
    url: String,
    token: Secret,
}

impl Config {
    pub(crate) fn load(explicit: Option<&Path>) -> Result<Self> {
        let paths = Paths::from_env()?;
        let mut builder = config::Config::builder();
        for path in layers(&paths.config_dir, explicit)? {
            builder = builder.add_source(config::File::from(path).format(config::FileFormat::Toml));
        }
        let file: File = builder
            .build()?
            .try_deserialize()
            .context("invalid server configuration")?;
        let server = file.server;
        let environment = server.environment.unwrap_or_else(|| String::from("local"));
        ensure!(
            valid_name(&environment),
            "invalid environment ID {environment:?}"
        );
        let listen = server
            .listen
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 7420)));
        ensure!(
            listen.ip().is_loopback() || server.token.is_some(),
            "non-loopback listeners require server.token"
        );
        ensure!(
            file.environments.is_empty() || server.token.is_some(),
            "linked environments require server.token"
        );
        for (id, harness) in &file.harnesses {
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
                    file.harnesses.contains_key(&name),
                    "unconfigured default harness {name:?}"
                );
                Some(name)
            }
            None if file.harnesses.len() == 1 => file.harnesses.keys().next().cloned(),
            None => None,
        };
        let mut environments = BTreeMap::new();
        for (name, peer) in file.environments {
            ensure!(
                valid_name(&name) && name != environment,
                "invalid peer environment ID {name:?}"
            );
            let url =
                Url::parse(&peer.url).with_context(|| format!("invalid peer URL for {name}"))?;
            ensure!(
                url.host().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "peer {name} must be a base URL without credentials or query parameters"
            );
            environments.insert(
                name,
                PeerConfig {
                    url,
                    token: peer.token.resolve()?,
                },
            );
        }
        let cwd = working_directory(server.cwd)?;
        Ok(Self {
            environment,
            cwd,
            state_dir: server.state_dir.unwrap_or(paths.state_dir),
            worktree_dir: server.worktree_dir,
            worktree_branch_prefix: server
                .worktree_branch_prefix
                .unwrap_or_else(|| "viz/".into()),
            listen,
            token: server.token,
            default_harness,
            harnesses: file.harnesses,
            environments,
        })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn layers(directory: &Path, explicit: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(path) = explicit.map(Path::to_path_buf).or_else(|| {
        env::var_os("VIZ_CONFIG")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }) {
        return Ok(vec![path]);
    }
    let mut paths = Vec::new();
    let main = directory.join("config.toml");
    if main.try_exists()? {
        paths.push(main);
    }
    let dropins = directory.join("config.d");
    let mut files = match fs::read_dir(&dropins) {
        Ok(files) => files
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(paths),
        Err(error) => bail!("reading {}: {error}", dropins.display()),
    };
    files.retain(|path| path.extension().is_some_and(|ext| ext == "toml"));
    files.sort_unstable();
    paths.extend(files);
    Ok(paths)
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
