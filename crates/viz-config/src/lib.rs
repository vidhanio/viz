//! Shared configuration for viz's Rust components.
//!
//! Each client reads only its own TOML file.

mod client_file;
mod connection;
mod error;
mod paths;
mod secret;

pub use crate::{
    client_file::{ClientFile, load},
    connection::{Connection, DEFAULT_URL},
    error::{Error, Result},
    paths::Paths,
    secret::{ResolveSecretError, Secret},
};
