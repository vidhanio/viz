//! Shared configuration for viz's Rust components.
//!
//! viz reads one TOML document, assembled from layers (later wins):
//!
//! 1. `$VIZ_CONFIG_DIR/config.toml` (default
//!    `$XDG_CONFIG_HOME/viz/config.toml`). Home Manager owns this file when the
//!    module is enabled.
//! 2. `$VIZ_CONFIG_DIR/config.d/*.toml`, in lexical order: hand-written or
//!    app-written overrides that survive a Home Manager switch.
//!
//! Tables merge recursively; any other value replaces the one below it. The
//! backend (TypeScript) implements the same layering in
//! `backend/src/config.ts`; keep the two in sync.
//!
//! Each component deserializes only the sections it owns. Connectors read
//! `[connectors.<name>]` through [`Config::connector`] into their own types.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, de::DeserializeOwned};

/// Errors from locating or reading configuration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Neither `$HOME` nor the relevant XDG variable is set.
    #[error("cannot locate {0}: set $HOME or ${0}")]
    NoHome(&'static str),
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
    /// A secret could not be resolved.
    #[error("resolving secret: {0}")]
    Secret(String),
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Filesystem locations viz uses, following the XDG base directory spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$VIZ_CONFIG_DIR`, default `$XDG_CONFIG_HOME/viz`.
    pub config_dir: PathBuf,
    /// `$XDG_STATE_HOME/viz`: databases and logs.
    pub state_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR/viz`, falling back to the state dir: sockets.
    pub runtime_dir: PathBuf,
}

impl Paths {
    /// Resolves paths from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoHome`] if a needed XDG variable and `$HOME` are both
    /// unset.
    pub fn from_env() -> Result<Self> {
        Self::resolve(|key| {
            env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
    }

    fn resolve(var: impl Fn(&str) -> Option<PathBuf>) -> Result<Self> {
        let xdg = |key: &'static str, fallback: &str| {
            var(key)
                .or_else(|| var("HOME").map(|home| home.join(fallback)))
                .ok_or(Error::NoHome(key))
        };
        let config_dir = match var("VIZ_CONFIG_DIR") {
            Some(dir) => dir,
            None => xdg("XDG_CONFIG_HOME", ".config")?.join("viz"),
        };
        let state_dir = xdg("XDG_STATE_HOME", ".local/state")?.join("viz");
        let runtime_dir =
            var("XDG_RUNTIME_DIR").map_or_else(|| state_dir.clone(), |dir| dir.join("viz"));
        Ok(Self {
            config_dir,
            state_dir,
            runtime_dir,
        })
    }

    /// The base layer, `config.toml`.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// The drop-in directory, `config.d`.
    #[must_use]
    pub fn config_dropins(&self) -> PathBuf {
        self.config_dir.join("config.d")
    }

    /// The backend's default local socket.
    #[must_use]
    pub fn backend_socket(&self) -> PathBuf {
        self.runtime_dir.join("backend.sock")
    }
}

/// A secret given inline, read from a file, or read from an environment
/// variable.
///
/// ```toml
/// bot_token = "inline"                         # discouraged outside throwaway setups
/// bot_token = { file = "/run/secrets/discord" } # sops-nix, agenix, systemd credentials
/// bot_token = { env = "DISCORD_TOKEN" }
/// ```
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

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    /// Returns [`Error::Secret`] if the file or variable is missing or empty.
    pub fn resolve(&self) -> Result<String> {
        let value = match self {
            Self::Inline(value) => value.clone(),
            Self::File { file } => fs::read_to_string(file)
                .map_err(|e| Error::Secret(format!("{}: {e}", file.display())))?
                .trim_end_matches(['\n', '\r'])
                .to_owned(),
            Self::Env { env } => {
                env::var(env).map_err(|e| Error::Secret(format!("${env}: {e}")))?
            }
        };
        if value.is_empty() {
            return Err(Error::Secret(format!("{self:?} is empty")));
        }
        Ok(value)
    }
}

/// `[backend]`: where the backend listens. Clients on the same machine use
/// these too.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BackendSection {
    /// Unix socket path. Default: [`Paths::backend_socket`].
    pub socket: Option<PathBuf>,
    /// Optional TCP `host:port` listener for remote clients (tailnet/LAN).
    /// Requires `token`.
    pub listen: Option<String>,
    /// Bearer token remote clients must present.
    pub token: Option<Secret>,
}

/// `[client]`: how Rust clients (connectors, desktop, CLI) reach the backend.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ClientSection {
    /// `http://host:port` of a remote backend. Unset: the local socket from `[backend]`.
    pub url: Option<String>,
    /// Token for `url`. Unset: `[backend].token`.
    pub token: Option<Secret>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Known {
    backend: BackendSection,
    client: ClientSection,
}

/// The merged configuration document.
#[derive(Debug, Clone)]
pub struct Config {
    /// Locations used to load this config.
    pub paths: Paths,
    /// `[backend]`.
    pub backend: BackendSection,
    /// `[client]`.
    pub client: ClientSection,
    doc: toml::Table,
}

impl Config {
    /// Loads and merges all layers from [`Paths::from_env`]. Missing files are
    /// not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if a layer can't be read or parsed, or
    /// `[backend]`/`[client]` are invalid.
    pub fn load() -> Result<Self> {
        Self::load_from(Paths::from_env()?)
    }

    /// Loads and merges all layers below `paths.config_dir`.
    ///
    /// # Errors
    ///
    /// See [`Config::load`].
    pub fn load_from(paths: Paths) -> Result<Self> {
        let mut doc = toml::Table::new();
        for layer in layers(&paths)? {
            merge(&mut doc, read_layer(&layer)?);
        }
        let known: Known = section_from(&doc, "")?;
        Ok(Self {
            paths,
            backend: known.backend,
            client: known.client,
            doc,
        })
    }

    /// The merged document.
    #[must_use]
    pub const fn document(&self) -> &toml::Table {
        &self.doc
    }

    /// Deserializes `[connectors.<name>]` (an empty table if absent) into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Section`] if the section doesn't match `T`.
    pub fn connector<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let section = self
            .doc
            .get("connectors")
            .and_then(|c| c.get(name))
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));
        section.try_into().map_err(|source| Error::Section {
            section: format!("connectors.{name}"),
            source: Box::new(source),
        })
    }

    /// The backend socket clients on this machine connect to.
    #[must_use]
    pub fn backend_socket(&self) -> PathBuf {
        self.backend
            .socket
            .clone()
            .unwrap_or_else(|| self.paths.backend_socket())
    }
}

fn section_from<T: DeserializeOwned>(doc: &toml::Table, name: &str) -> Result<T> {
    toml::Value::Table(doc.clone())
        .try_into()
        .map_err(|source| Error::Section {
            section: name.to_owned(),
            source: Box::new(source),
        })
}

fn layers(paths: &Paths) -> Result<Vec<PathBuf>> {
    let mut layers = vec![paths.config_file()];
    let dropins = paths.config_dropins();
    match fs::read_dir(&dropins) {
        Ok(entries) => {
            let mut found = entries
                .map(|entry| entry.map(|e| e.path()))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|source| Error::Read {
                    path: dropins.clone(),
                    source,
                })?;
            found.retain(|p| p.extension().is_some_and(|ext| ext == "toml"));
            found.sort();
            layers.extend(found);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::Read {
                path: dropins,
                source,
            });
        }
    }
    Ok(layers)
}

fn read_layer(path: &Path) -> Result<toml::Table> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(toml::Table::new()),
        Err(source) => {
            return Err(Error::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    text.parse().map_err(|source| Error::Parse {
        path: path.to_owned(),
        source: Box::new(source),
    })
}

/// Merges `top` into `base`: tables recursively, everything else replaced.
fn merge(base: &mut toml::Table, top: toml::Table) {
    for (key, value) in top {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(top)) => merge(base, top),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("viz-config-test-{name}-{}", std::process::id()));
        _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("config.d")).unwrap();
        dir
    }

    fn paths(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.to_owned(),
            state_dir: dir.join("state"),
            runtime_dir: dir.join("run"),
        }
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Discord {
        bot_token: Secret,
        #[serde(default)]
        allowed_users: Vec<u64>,
        enable: bool,
    }

    #[test]
    fn dropins_override_base_and_merge_tables() {
        let dir = temp_dir("merge");
        fs::write(
            dir.join("config.toml"),
            "[backend]\nlisten = \"127.0.0.1:1\"\n[connectors.discord]\nenable = true\nbot_token = { file = \"/run/secrets/discord\" }\nallowed_users = [1]\n",
        )
        .unwrap();
        fs::write(
            dir.join("config.d/10-a.toml"),
            "[connectors.discord]\nallowed_users = [2, 3]\n",
        )
        .unwrap();
        fs::write(
            dir.join("config.d/20-b.toml"),
            "[connectors.discord]\nenable = false\n",
        )
        .unwrap();
        fs::write(dir.join("config.d/ignored.txt"), "not = [toml").unwrap();

        let config = Config::load_from(paths(&dir)).unwrap();
        assert_eq!(config.backend.listen.as_deref(), Some("127.0.0.1:1"));
        let discord: Discord = config.connector("discord").unwrap();
        assert_eq!(
            discord,
            Discord {
                bot_token: Secret::File {
                    file: "/run/secrets/discord".into()
                },
                allowed_users: vec![2, 3],
                enable: false,
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_config_is_empty() {
        let dir = temp_dir("missing");
        fs::remove_dir_all(&dir).unwrap();
        let config = Config::load_from(paths(&dir)).unwrap();
        assert!(config.document().is_empty());
        assert_eq!(config.backend_socket(), dir.join("run/backend.sock"));
    }

    #[test]
    fn xdg_resolution() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| PathBuf::from(v))
            }
        };
        let paths =
            Paths::resolve(env(&[("HOME", "/h"), ("XDG_RUNTIME_DIR", "/run/user/1")])).unwrap();
        assert_eq!(paths.config_file(), Path::new("/h/.config/viz/config.toml"));
        assert_eq!(paths.state_dir, Path::new("/h/.local/state/viz"));
        assert_eq!(
            paths.backend_socket(),
            Path::new("/run/user/1/viz/backend.sock")
        );

        let paths = Paths::resolve(env(&[("HOME", "/h"), ("VIZ_CONFIG_DIR", "/etc/viz")])).unwrap();
        assert_eq!(paths.config_dir, Path::new("/etc/viz"));
        assert_eq!(paths.runtime_dir, Path::new("/h/.local/state/viz"));

        assert!(Paths::resolve(env(&[])).is_err());
    }

    #[test]
    fn secret_forms() {
        #[derive(Deserialize)]
        struct S {
            a: Secret,
            b: Secret,
            c: Secret,
        }
        let s: S = toml::from_str("a = \"x\"\nb = { file = \"/f\" }\nc = { env = \"E\" }").unwrap();
        assert_eq!(s.a.resolve().unwrap(), "x");
        assert_eq!(s.b, Secret::File { file: "/f".into() });
        assert_eq!(s.c, Secret::Env { env: "E".into() });
        assert_eq!(format!("{:?}", s.a), "Secret(<inline>)");
    }
}
