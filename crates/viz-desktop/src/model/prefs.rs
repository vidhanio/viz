//! Versioned, tolerant client-local preferences schema persisted to
//! `$XDG_STATE_HOME/viz/desktop.json`.
//!
//! Server-shared state (pins, order, settle, snooze, project settings, unread)
//! lives in the viz server database. This schema holds only client-local UI
//! preferences: panel widths, sidebar open state, collapsed shelves, favorite
//! models, window geometry, and unsent drafts.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::sidebar::{SHELF_SETTLED, SHELF_SNOOZED};

/// Current schema version written to `desktop.json`.
pub const PREFS_VERSION: u32 = 1;

/// Default collapsed shelves on a fresh workspace (Working starts open;
/// Snoozed and Settled start collapsed).
#[must_use]
pub fn default_collapsed_shelves() -> BTreeSet<String> {
    BTreeSet::from([SHELF_SNOOZED.to_owned(), SHELF_SETTLED.to_owned()])
}

/// Persisted window dimensions in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowGeometry {
    /// Window width in points.
    pub width: f32,
    /// Window height in points.
    pub height: f32,
}

/// Client-local desktop preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Schema version (currently [`PREFS_VERSION`]).
    pub version: u32,
    /// Saved sidebar width in points.
    pub sidebar_width: Option<f32>,
    /// Whether the sidebar is open.
    pub sidebar_open: Option<bool>,
    /// Saved docked diff panel width in points.
    pub diff_width: Option<f32>,
    /// Saved docked terminal height in points.
    pub terminal_height: Option<f32>,
    /// Shelf names collapsed in the sidebar triage view.
    pub collapsed_shelves: BTreeSet<String>,
    /// User-starred model identifiers (`harness/model` or model id).
    pub favorite_models: Vec<String>,
    /// Unsent draft text keyed by environment id then thread id string.
    pub drafts: BTreeMap<String, BTreeMap<String, String>>,
    /// Last recorded window size.
    pub window: Option<WindowGeometry>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            version: PREFS_VERSION,
            sidebar_width: None,
            sidebar_open: None,
            diff_width: None,
            terminal_height: None,
            collapsed_shelves: default_collapsed_shelves(),
            favorite_models: Vec::new(),
            drafts: BTreeMap::new(),
            window: None,
        }
    }
}

impl Prefs {
    /// Parses `desktop.json` tolerantly: invalid JSON returns defaults, unknown
    /// keys are ignored, and any individually malformed field falls back to its
    /// default without discarding sibling preferences.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(bytes) else {
            return Self::default();
        };
        let version = root
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(PREFS_VERSION);
        let sidebar_width = positive_f32(&root, "sidebarWidth");
        let sidebar_open = root.get("sidebarOpen").and_then(Value::as_bool);
        let diff_width = positive_f32(&root, "diffWidth");
        let terminal_height = positive_f32(&root, "terminalHeight");
        let collapsed_shelves = root
            .get("collapsedShelves")
            .and_then(Value::as_array)
            .map_or_else(default_collapsed_shelves, |items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        let favorite_models = string_vec(&root, "favoriteModels");
        let drafts = parse_drafts(&root);
        let window = parse_window(&root);
        Self {
            version,
            sidebar_width,
            sidebar_open,
            diff_width,
            terminal_height,
            collapsed_shelves,
            favorite_models,
            drafts,
            window,
        }
    }

    /// Serializes preferences deterministically with the current schema
    /// version.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut root = Map::new();
        root.insert("version".into(), json!(PREFS_VERSION));
        if let Some(width) = self.sidebar_width.filter(|w| w.is_finite() && *w > 0.0) {
            root.insert("sidebarWidth".into(), json!(width));
        }
        if let Some(open) = self.sidebar_open {
            root.insert("sidebarOpen".into(), json!(open));
        }
        if let Some(width) = self.diff_width.filter(|w| w.is_finite() && *w > 0.0) {
            root.insert("diffWidth".into(), json!(width));
        }
        if let Some(height) = self.terminal_height.filter(|h| h.is_finite() && *h > 0.0) {
            root.insert("terminalHeight".into(), json!(height));
        }
        let collapsed_shelves: Vec<&str> =
            self.collapsed_shelves.iter().map(String::as_str).collect();
        root.insert("collapsedShelves".into(), json!(collapsed_shelves));
        if !self.favorite_models.is_empty() {
            root.insert("favoriteModels".into(), json!(&self.favorite_models));
        }
        if !self.drafts.is_empty() {
            root.insert("drafts".into(), json!(&self.drafts));
        }
        if let Some(window) = self.window.filter(|w| {
            w.width.is_finite() && w.height.is_finite() && w.width > 0.0 && w.height > 0.0
        }) {
            root.insert(
                "window".into(),
                json!({ "width": window.width, "height": window.height }),
            );
        }
        Value::Object(root).to_string()
    }
}

fn positive_f32(root: &Map<String, Value>, key: &str) -> Option<f32> {
    let raw = root.get(key)?.as_f64()?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "UI layout dimensions in points fit safely in f32"
    )]
    let value = raw as f32;
    (value.is_finite() && value > 0.0).then_some(value)
}

fn string_vec(root: &Map<String, Value>, key: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in root
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if seen.insert(item) {
            out.push(item.to_owned());
        }
    }
    out
}

fn parse_drafts(root: &Map<String, Value>) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    let Some(Value::Object(envs)) = root.get("drafts") else {
        return out;
    };
    for (env, value) in envs {
        let Some(threads) = value.as_object() else {
            continue;
        };
        let mut env_drafts = BTreeMap::new();
        for (thread, text) in threads {
            if let Some(text) = text.as_str().filter(|text| !text.is_empty()) {
                env_drafts.insert(thread.clone(), text.to_owned());
            }
        }
        if !env_drafts.is_empty() {
            out.insert(env.clone(), env_drafts);
        }
    }
    out
}

fn parse_window(root: &Map<String, Value>) -> Option<WindowGeometry> {
    let obj = root.get("window")?.as_object()?;
    let width = positive_f32(obj, "width")?;
    let height = positive_f32(obj, "height")?;
    Some(WindowGeometry { width, height })
}
