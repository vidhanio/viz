//! Harness launch configuration.

use std::collections::BTreeMap;

use serde::Deserialize;
use viz_config::Secret;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    pub name: Option<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, Secret>,
    pub model: Option<String>,
    pub effort: Option<String>,
}
