//! Where viz keeps its config and state.

use std::{env, path::PathBuf};

use crate::{Error, Result};

/// Filesystem locations viz uses, following the XDG base directory spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$VIZ_CONFIG_DIR`, default `$XDG_CONFIG_HOME/viz`.
    pub config_dir: PathBuf,
    /// `$STATE_DIRECTORY` (systemd `StateDirectory=`), default
    /// `$XDG_STATE_HOME/viz`: databases and logs.
    pub state_dir: PathBuf,
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
        let config_dir = config_dir()?;
        let state_dir = match var("STATE_DIRECTORY") {
            Some(dir) => dir,
            None => xdg("XDG_STATE_HOME", ".local/state")?.join("viz"),
        };
        Ok(Self {
            config_dir,
            state_dir,
        })
    }
}

pub fn config_dir() -> Result<PathBuf> {
    match var("VIZ_CONFIG_DIR") {
        Some(dir) => Ok(dir),
        None => Ok(xdg("XDG_CONFIG_HOME", ".config")?.join("viz")),
    }
}

fn var(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn xdg(key: &'static str, fallback: &str) -> Result<PathBuf> {
    var(key)
        .or_else(|| var("HOME").map(|home| home.join(fallback)))
        .ok_or(Error::NoHome { var: key })
}
