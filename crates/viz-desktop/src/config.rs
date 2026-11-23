//! The `[desktop]` table of the shared config file: the server connection
//! (shared `[client.server]`) and the theme. The table is optional; without it
//! the desktop uses the default HTTP origin in the default schemes.

use serde::Deserialize;

use crate::theme::ThemeConfig;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopConfig {
    pub theme: ThemeConfig,
}
