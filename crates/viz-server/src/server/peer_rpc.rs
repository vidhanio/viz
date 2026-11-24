//! Peer-only method registry. A peer payload's claimed source is checked
//! against the authenticated environment before invoking a typed handler.

use futures_util::future::BoxFuture;
use serde_json::Value;
use viz_protocol::peer::{FollowUp, PeerMethod, ProgressRequest, ReceiveTask, RetractTask};

use super::Server;
use crate::{
    error::Error,
    rpc::{self, Handler},
    wire::encode,
};

pub(super) enum PeerError {
    Rpc(Error),
    SourceMismatch,
}

impl From<Error> for PeerError {
    fn from(error: Error) -> Self {
        Self::Rpc(error)
    }
}

type Dispatch = for<'a> fn(&'a Server, &'a str, &'a str) -> BoxFuture<'a, Result<Value, PeerError>>;

fn dispatch<'a, M: PeerMethod>(
    server: &'a Server,
    raw: &'a str,
    source: &'a str,
) -> BoxFuture<'a, Result<Value, PeerError>>
where
    Server: Handler<M>,
{
    Box::pin(async move {
        let input: M = rpc::parse(raw)?;
        if input.source().is_some_and(|claimed| claimed != source) {
            return Err(PeerError::SourceMismatch);
        }
        Ok(encode(server.handle(input).await?)?)
    })
}

macro_rules! peer_handlers {
    ($($method:ty => $service:ident.$operation:ident),+ $(,)?) => {
        const ROUTES: &[(&str, Dispatch)] = &[
            $((<$method as viz_protocol::Method>::NAME, dispatch::<$method>)),+
        ];

        $(
            impl Handler<$method> for Server {
                async fn handle(&self, input: $method) -> Result<
                    <$method as viz_protocol::Method>::Response,
                    Error,
                > {
                    self.$service.$operation(input).await
                }
            }
        )+
    };
}

peer_handlers! {
    ReceiveTask => workspace.receive,
    RetractTask => workspace.retract,
    ProgressRequest => tasks.progress,
    FollowUp => tasks.follow_up,
}

pub(super) fn find(method: &str) -> Option<Dispatch> {
    ROUTES
        .iter()
        .find(|(name, _)| *name == method)
        .map(|(_, dispatch)| *dispatch)
}
