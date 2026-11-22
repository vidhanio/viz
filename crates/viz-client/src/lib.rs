//! Client for the viz server: JSON-RPC 2.0 over a WebSocket, with an optional
//! bearer token.

mod client;
mod endpoint;
mod error;
pub use viz_protocol as protocol;

pub use crate::{
    client::{Client, Connection},
    endpoint::{Endpoint, ResolveEndpointError},
    error::{Error, Result},
};
