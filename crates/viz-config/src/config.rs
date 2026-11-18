//! The merged configuration document and how its layers are found, read and
//! merged.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::de::DeserializeOwned;

use crate::{BackendSection, ClientSection, Error, Paths, Result};

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
    /// Returns an error if a location can't be resolved, a layer can't be read
    /// or parsed, or `[backend]`/`[client]` are invalid.
    pub fn load() -> Result<Self> {
        Self::load_from(Paths::from_env()?)
    }

    /// Loads and merges all layers below `paths.config_dir` (or just
    /// `paths.config_override`).
    ///
    /// # Errors
    ///
    /// Returns an error if a layer can't be read or parsed, or
    /// `[backend]`/`[client]` are invalid.
    pub fn load_from(paths: Paths) -> Result<Self> {
        let mut doc = toml::Table::new();
        for layer in layers(&paths)? {
            merge(&mut doc, read_layer(&layer)?);
        }
        Ok(Self {
            backend: section(&doc, &["backend"])?,
            client: section(&doc, &["client"])?,
            paths,
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
        section(&self.doc, &["connectors", name])
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

/// Deserializes the table at the dotted `keys` (an empty table if absent) into
/// `T`.
fn section<T: DeserializeOwned>(doc: &toml::Table, keys: &[&str]) -> Result<T> {
    let mut found = None;
    let mut table = Some(doc);
    for key in keys {
        found = table.and_then(|table| table.get(*key));
        table = found.and_then(toml::Value::as_table);
    }
    let value = found
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));
    value.try_into().map_err(|source| Error::Section {
        section: keys.join("."),
        source: Box::new(source),
    })
}

/// The layer files to read, lowest first.
fn layers(paths: &Paths) -> Result<Vec<PathBuf>> {
    if let Some(file) = &paths.config_override {
        if !file.is_file() {
            return Err(Error::Read {
                path: file.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, "$VIZ_CONFIG names no file"),
            });
        }
        return Ok(vec![file.clone()]);
    }
    let mut layers = vec![paths.config_file()];
    let dropins = paths.config_dropins();
    match fs::read_dir(&dropins) {
        Ok(entries) => {
            let mut found = entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|source| Error::Read {
                    path: dropins.clone(),
                    source,
                })?;
            found.retain(|path| path.extension().is_some_and(|ext| ext == "toml"));
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

/// One layer's table; a missing file is an empty one.
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
