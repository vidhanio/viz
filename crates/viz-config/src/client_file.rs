//! Locations and loading for individual client configuration files.
//!
//! Each client loads one file: an explicit `--config` path takes precedence
//! over its per-client environment variable and the default under
//! `$VIZ_CONFIG_DIR` (or `$XDG_CONFIG_HOME/viz`, default `~/.config/viz`).
//! Unlike the backend, clients do not layer `config.d` drop-ins. A missing
//! file yields defaults; client-specific requirements still apply.

use std::{env, fs, io, path::PathBuf};

use serde::de::DeserializeOwned;

use crate::{Error, Result, paths::config_dir};

/// Selects a client file by component or by explicit path.
#[derive(Debug, Clone)]
pub struct ClientFile(Location);

#[derive(Debug, Clone)]
enum Location {
    Desktop,
    Cli,
    Connector(String),
    Explicit(PathBuf),
}

impl ClientFile {
    /// Desktop file, overridden by `$VIZ_DESKTOP_CONFIG`.
    #[must_use]
    pub const fn desktop() -> Self {
        Self(Location::Desktop)
    }

    /// CLI file, overridden by `$VIZ_CLI_CONFIG`.
    #[must_use]
    pub const fn cli() -> Self {
        Self(Location::Cli)
    }

    /// Named connector file, overridden by `$VIZ_<NAME>_CONFIG`.
    #[must_use]
    pub fn connector(name: &str) -> Self {
        Self(Location::Connector(name.to_owned()))
    }

    /// An explicit path, taking precedence over all environment overrides.
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self(Location::Explicit(path.into()))
    }

    /// Resolves the selected file without opening it.
    ///
    /// # Errors
    /// Returns an error if the default config directory cannot be located.
    pub fn resolve(&self) -> Result<PathBuf> {
        let key = match &self.0 {
            Location::Desktop => "VIZ_DESKTOP_CONFIG",
            Location::Cli => "VIZ_CLI_CONFIG",
            Location::Connector(name) => &format!("VIZ_{}_CONFIG", name.to_ascii_uppercase()),
            Location::Explicit(path) => return Ok(path.clone()),
        };
        if let Some(path) = env::var_os(key).filter(|value| !value.is_empty()) {
            return Ok(path.into());
        }
        let dir = config_dir()?;
        Ok(match &self.0 {
            Location::Desktop => dir.join("desktop.toml"),
            Location::Cli => dir.join("cli.toml"),
            Location::Connector(name) => dir.join("connectors").join(format!("{name}.toml")),
            Location::Explicit(_) => unreachable!(),
        })
    }
}

/// Loads the selected TOML file, returning defaults if it does not exist.
///
/// # Errors
/// Returns an error if the location cannot be resolved or the file cannot be
/// read or deserialized.
pub fn load<T: DeserializeOwned + Default>(file: &ClientFile) -> Result<T> {
    let path = file.resolve()?;
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|source| Error::Parse {
            path,
            source: Box::new(source),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(Error::Read { path, source }),
    }
}
