//! `desktop.toml`: the server connection and the theme. The file is optional;
//! without it the desktop uses the default HTTP origin in the default schemes.

use serde::Deserialize;

use crate::theme::ThemeConfig;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopConfig {
    pub server: viz_config::Connection,
    pub theme: ThemeConfig,
}
