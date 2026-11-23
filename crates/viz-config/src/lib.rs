//! Shared configuration for viz's Rust components.
//!
//! One TOML file; each component reads only its own table.

mod connection;
mod error;
mod file;
mod paths;
mod secret;

pub use crate::{
    connection::{Connection, DEFAULT_URL},
    error::{Error, Result},
    file::Component,
    paths::Paths,
    secret::{ResolveSecretError, Secret},
};
