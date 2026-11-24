//! Peer-only delegation routes registered into the unified RPC router.
//!
//! Each route carries [`viz_protocol::MethodScope::PEER_DELEGATION`] and binds
//! any claimed source environment in the payload to the authenticated peer
//! principal in [`CallCtx`].

use viz_protocol::peer::{FollowUp, PeerMethod, ProgressRequest, ReceiveTask, RetractTask};

use super::Server;
use crate::{
    error::{Error, RequestError},
    rpc::{CallCtx, Handler, Route},
};

fn verify_source<M: PeerMethod>(ctx: &CallCtx, input: &M) -> Result<(), Error> {
    let Some(source) = ctx.peer() else {
        return Err(RequestError::Forbidden("peer credential required".into()).into());
    };
    if input.source().is_some_and(|claimed| claimed != source) {
        return Err(RequestError::Forbidden("source environment mismatch".into()).into());
    }
    Ok(())
}

macro_rules! peer_handlers {
    ($($method:ty => $service:ident.$operation:ident),+ $(,)?) => {
        pub(super) const ROUTES: &[Route<Server>] = &[
            $(Route::new::<$method>()),+
        ];

        $(
            impl Handler<$method> for Server {
                async fn handle(
                    &self,
                    ctx: &CallCtx,
                    input: $method,
                ) -> Result<<$method as viz_protocol::Method>::Response, Error> {
                    verify_source(ctx, &input)?;
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
