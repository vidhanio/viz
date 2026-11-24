//! Shared configuration for viz's Rust components.
//!
//! One TOML file; each component reads only its own table.

mod connection;
mod error;
mod file;
mod paths;
mod secret;

pub use crate::{
    connection::{Client, Connection},
    error::{Error, Result},
    file::Component,
    paths::{MAX_UNIX_SOCKET_PATH, Paths, default_socket, validate_socket_path},
    secret::{ResolveSecretError, Secret},
};
