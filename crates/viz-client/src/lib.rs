//! Client for the viz backend: JSON-RPC 2.0 over a WebSocket, with an optional
//! bearer token.

mod client;
mod endpoint;
mod error;
pub mod protocol;

pub use crate::{
    client::{Client, Connection},
    endpoint::{Endpoint, ResolveEndpointError},
    error::{Error, Result},
};
