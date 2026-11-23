//! Locating and loading viz's single TOML configuration file.
//!
//! Every component reads the same file and deserializes only its own top-level
//! table (`[server]`, `[desktop]`, `[cli]`). The file location is, in order: an
//! explicit `--config` path, the component's environment variable
//! (`$VIZ_SERVER_CONFIG`, `$VIZ_DESKTOP_CONFIG`, `$VIZ_CLI_CONFIG`), then
//! `$XDG_CONFIG_HOME/viz/config.toml` (default `~/.config/viz/config.toml`).
//! A missing file or table yields defaults; component-specific requirements
//! still apply.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

use crate::{
    Error, Result,
    paths::{config_dir, var},
};

/// A viz program that reads its own table of the shared config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// `viz-server`, table `[server]`.
    Server,
    /// `viz-desktop`, table `[desktop]`.
    Desktop,
    /// The `viz` command, table `[cli]`.
    Cli,
}

impl Component {
    /// The top-level table this component reads.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Desktop => "desktop",
            Self::Cli => "cli",
        }
    }

    /// The environment variable that overrides the config file path.
    #[must_use]
    pub const fn env_var(self) -> &'static str {
        match self {
            Self::Server => "VIZ_SERVER_CONFIG",
            Self::Desktop => "VIZ_DESKTOP_CONFIG",
            Self::Cli => "VIZ_CLI_CONFIG",
        }
    }

    /// Resolves the config file path without opening it.
    ///
    /// # Errors
    /// Returns an error if the default config directory cannot be located.
    pub fn path(self, explicit: Option<&Path>) -> Result<PathBuf> {
        if let Some(path) = explicit {
            return Ok(path.to_path_buf());
        }
        match var(self.env_var()) {
            Some(path) => Ok(path),
            None => Ok(config_dir()?.join("config.toml")),
        }
    }

    /// Loads this component's table, returning defaults if the file or the
    /// table does not exist.
    ///
    /// # Errors
    /// Returns an error if the location cannot be resolved or the file cannot
    /// be read or deserialized.
    pub fn load<T: DeserializeOwned + Default>(self, explicit: Option<&Path>) -> Result<T> {
        let path = self.path(explicit)?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(source) => return Err(Error::Read { path, source }),
        };
        let parse = |source| Error::Parse {
            path: path.clone(),
            source: Box::new(source),
        };
        let mut file: toml::Table = toml::from_str(&text).map_err(parse)?;
        file.remove(self.table())
            .map_or_else(|| Ok(T::default()), |table| table.try_into().map_err(parse))
    }
}
