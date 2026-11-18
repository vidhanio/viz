//! Shared configuration for viz's Rust components.
//!
//! viz reads one TOML document, assembled from layers (later wins):
//!
//! 1. `$VIZ_CONFIG_DIR/config.toml` (default
//!    `$XDG_CONFIG_HOME/viz/config.toml`). Home Manager owns this file when the
//!    module is enabled.
//! 2. `$VIZ_CONFIG_DIR/config.d/*.toml`, in lexical order: hand-written or
//!    app-written overrides that survive a Home Manager switch.
//!
//! Tables merge recursively; any other value replaces the one below it.
//! `$VIZ_CONFIG` names one file that replaces all layers (e.g. the repo's
//! gitignored `viz.local.toml` for development). The backend (TypeScript)
//! implements the same layering in `backend/src/config.ts`; keep the two in
//! sync.
//!
//! Each component deserializes only the sections it owns. Connectors read
//! `[connectors.<name>]` through [`Config::connector`] into their own types.

mod config;
mod error;
mod paths;
mod secret;
mod section;

pub use crate::{
    config::Config,
    error::{Error, Result},
    paths::Paths,
    secret::{ResolveSecretError, Secret},
    section::{BackendSection, ClientSection},
};
