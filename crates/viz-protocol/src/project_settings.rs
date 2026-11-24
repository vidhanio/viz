//! Editable project preferences and icon responses.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::OptionValue;

/// Stored project preferences. Optional defaults inherit from the environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSettings {
    pub icon: ProjectIcon,
    pub pinned: bool,
    pub order: Option<i64>,
    pub archived: bool,
    pub default_harness: Option<String>,
    pub default_model: Option<String>,
    pub default_options: BTreeMap<String, OptionValue>,
    pub default_checkout: DefaultCheckout,
    pub base_branch: Option<String>,
    pub branch_prefix: Option<String>,
    pub additional_directories: Vec<String>,
    pub auto_settle: Option<ProjectAutoSettle>,
}

/// A patch omits untouched fields. Nullable preferences use explicit set or
/// clear.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSettingsPatch {
    pub icon: Option<ProjectIconChange>,
    pub pinned: Option<bool>,
    pub order: Option<Setting<i64>>,
    pub archived: Option<bool>,
    pub default_harness: Option<Setting<String>>,
    pub default_model: Option<Setting<String>>,
    pub default_options: Option<BTreeMap<String, OptionValue>>,
    pub default_checkout: Option<DefaultCheckout>,
    pub base_branch: Option<Setting<String>>,
    pub branch_prefix: Option<Setting<String>>,
    pub additional_directories: Option<Vec<String>>,
    pub auto_settle: Option<Setting<ProjectAutoSettle>>,
}

/// Replace a nullable preference or resume inheriting its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", content = "value", rename_all = "camelCase")]
pub enum Setting<T> {
    Set(T),
    Clear,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DefaultCheckout {
    #[default]
    Local,
    NewWorktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectAutoSettle {
    Never,
    AfterDays { days: u16 },
}

/// Icon selection advertised in project summaries. Image bytes are read
/// separately.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProjectIcon {
    #[default]
    Automatic,
    OwnerAvatar,
    Lucide {
        name: String,
        color: String,
    },
    Emoji {
        emoji: String,
    },
    Uploaded {
        media_type: String,
        /// Lowercase SHA-256 hex digest stored in the workspace blob store.
        blob: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProjectIconChange {
    Automatic,
    OwnerAvatar,
    Lucide {
        name: String,
        color: String,
    },
    Emoji {
        emoji: String,
    },
    /// Content-addressed image blob staged via the content WebSocket.
    Uploaded {
        media_type: String,
        blob: String,
    },
}
