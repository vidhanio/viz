//! Tolerant, atomic persistence for `$XDG_STATE_HOME/viz/desktop.json`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::model::Prefs;

/// Returns `$XDG_STATE_HOME/viz/desktop.json` when XDG paths are available.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    viz_config::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir.join("desktop.json"))
}

/// Reads and parses preferences from `path`, falling back to defaults when
/// missing or unreadable.
#[must_use]
pub fn load(path: &Path) -> Prefs {
    fs::read(path)
        .ok()
        .map_or_else(Prefs::default, |bytes| Prefs::from_bytes(&bytes))
}

/// Writes `prefs` atomically to `path` via a sibling temporary file. Failures
/// are logged at debug level and ignored so persistence errors never interrupt
/// the UI.
pub fn save(path: &Path, prefs: &Prefs) {
    let Some(dir) = path.parent() else {
        return;
    };
    if let Err(error) = fs::create_dir_all(dir) {
        tracing::debug!(path = %dir.display(), %error, "could not create desktop state directory");
        return;
    }
    let tmp = dir.join(format!(".desktop.json.tmp.{}", std::process::id()));
    let encoded = prefs.to_json();
    if let Err(error) = fs::write(&tmp, encoded) {
        tracing::debug!(path = %tmp.display(), %error, "could not write temporary desktop prefs");
        return;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        tracing::debug!(path = %path.display(), %error, "could not commit desktop prefs");
        _ = fs::remove_file(&tmp);
    }
}
