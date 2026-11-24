//! Client for the viz server: JSON-RPC 2.0 over a WebSocket, with an optional
//! bearer token.

mod client;
mod content;
mod endpoint;
mod error;
pub use viz_protocol as protocol;

pub use crate::{
    client::{Client, Connection},
    content::{ContentClient, DownloadedContent, ProjectIconFetch},
    endpoint::{Endpoint, ResolveEndpointError},
    error::{Error, Result},
};
