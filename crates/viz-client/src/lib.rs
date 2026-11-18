//! Client for the viz backend: JSON-RPC 2.0 over a WebSocket, on the local Unix
//! socket or a remote `http://host:port` (tailnet/LAN) with a bearer token.

mod client;
pub mod delta;
mod endpoint;
mod error;
pub mod protocol;

pub use crate::{
    client::{Client, Connection},
    endpoint::{Endpoint, ResolveEndpointError},
    error::{Error, Result},
};
