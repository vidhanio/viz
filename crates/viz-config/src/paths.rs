//! Where viz keeps its config, state and sockets.

use std::{env, path::PathBuf};

use crate::{Error, Result};

/// Filesystem locations viz uses, following the XDG base directory spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$VIZ_CONFIG`: the one config file to read instead of the layers.
    pub config_override: Option<PathBuf>,
    /// `$VIZ_CONFIG_DIR`, default `$XDG_CONFIG_HOME/viz`.
    pub config_dir: PathBuf,
    /// `$STATE_DIRECTORY` (systemd `StateDirectory=`), default
    /// `$XDG_STATE_HOME/viz`: databases and logs.
    pub state_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR/viz`, falling back to the state dir: sockets.
    pub runtime_dir: PathBuf,
}

impl Paths {
    /// Resolves paths from the process environment. Empty variables count as
    /// unset.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoHome`] if a needed XDG variable and `$HOME` are both
    /// unset.
    pub fn from_env() -> Result<Self> {
        let var = |key: &str| {
            env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let xdg = |key: &'static str, fallback: &str| {
            var(key)
                .or_else(|| var("HOME").map(|home| home.join(fallback)))
                .ok_or(Error::NoHome { var: key })
        };
        let config_dir = match var("VIZ_CONFIG_DIR") {
            Some(dir) => dir,
            None => xdg("XDG_CONFIG_HOME", ".config")?.join("viz"),
        };
        let state_dir = match var("STATE_DIRECTORY") {
            Some(dir) => dir,
            None => xdg("XDG_STATE_HOME", ".local/state")?.join("viz"),
        };
        let runtime_dir =
            var("XDG_RUNTIME_DIR").map_or_else(|| state_dir.clone(), |dir| dir.join("viz"));
        Ok(Self {
            config_override: var("VIZ_CONFIG"),
            config_dir,
            state_dir,
            runtime_dir,
        })
    }

    /// The base layer: `$VIZ_CONFIG`, else `config.toml`.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_override
            .clone()
            .unwrap_or_else(|| self.config_dir.join("config.toml"))
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
