//! Typed JSON-RPC dispatch with capability-scoped authorization.
//!
//! Only the transport adapter decodes input and encodes results; every route
//! enforces [`viz_protocol::Method::SCOPE`] against the caller's authenticated
//! [`CallCtx`] before parsing params or invoking its typed [`Handler`].

use std::{future::Future, sync::Arc};

use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::Value;
use viz_protocol::{ClientInfo, ClientKind, Method, MethodScope};

use crate::{
    error::{Error, RequestError},
    wire::encode,
};

/// Authenticated transport principal injected by the Unix/TCP HTTP guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Principal {
    /// Local user connected over the `0600` Unix domain socket.
    Local,
    /// Remote operator authenticated with `server.token` over TCP.
    Client,
    /// Configured peer environment authenticated with its per-peer `accept`
    /// credential over TCP.
    Peer(Arc<str>),
}

impl Principal {
    /// Stable principal key scoping staged content-plane blob leases.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Client => "client".to_owned(),
            Self::Peer(id) => format!("peer:{id}"),
        }
    }
}

/// Authenticated caller context threaded through every RPC dispatch and
/// content-plane session.
#[derive(Debug, Clone)]
pub(crate) struct CallCtx {
    pub(crate) principal: Principal,
    pub(crate) scope: MethodScope,
    pub(crate) client: Option<ClientInfo>,
}

impl CallCtx {
    /// Context for an authenticated WebSocket session after `hello`.
    pub(crate) const fn websocket(principal: Principal, client: ClientInfo) -> Self {
        let scope = match client.kind {
            ClientKind::Connector => MethodScope::CONNECTOR,
            ClientKind::App | ClientKind::Cli => MethodScope::OPERATOR,
        };
        Self {
            principal,
            scope,
            client: Some(client),
        }
    }

    /// Context for an authenticated peer calling `POST /v2/rpc`.
    pub(crate) const fn peer_http(source: Arc<str>) -> Self {
        Self {
            principal: Principal::Peer(source),
            scope: MethodScope::PEER_DELEGATION.union(MethodScope::PEER_TOOL),
            client: None,
        }
    }

    /// Context for trusted in-process MCP tool dispatch.
    pub(crate) const fn internal() -> Self {
        Self {
            principal: Principal::Local,
            scope: MethodScope::OPERATOR,
            client: None,
        }
    }

    /// Connector name if this session authenticated as a
    /// [`ClientKind::Connector`].
    pub(crate) fn connector(&self) -> Option<&str> {
        self.client
            .as_ref()
            .filter(|client| client.kind == ClientKind::Connector)
            .map(|client| client.name.as_str())
    }

    /// Authenticated peer environment ID if this caller authenticated with a
    /// per-peer `accept` credential.
    pub(crate) fn peer(&self) -> Option<&str> {
        match &self.principal {
            Principal::Peer(source) => Some(source),
            Principal::Local | Principal::Client => None,
        }
    }

    /// Verifies that this caller holds at least one capability bit in
    /// `required`.
    pub(crate) fn require(&self, required: MethodScope, label: &str) -> Result<(), Error> {
        if self.scope.intersects(required) {
            Ok(())
        } else {
            Err(
                RequestError::Forbidden(format!("{label} is not permitted for this session"))
                    .into(),
            )
        }
    }
}

pub(crate) trait Handler<M: Method> {
    fn handle(
        &self,
        ctx: &CallCtx,
        input: M,
    ) -> impl Future<Output = Result<M::Response, Error>> + Send;
}

type Dispatch<C> = for<'a> fn(&'a C, &'a CallCtx, &'a str) -> BoxFuture<'a, Result<Value, Error>>;

#[derive(Debug)]
pub(crate) struct Route<C> {
    name: &'static str,
    scope: MethodScope,
    dispatch: Dispatch<C>,
}

impl<C: 'static> Route<C> {
    pub(crate) const fn new<M: Method>() -> Self
    where
        C: Handler<M> + Sync,
    {
        fn call<'a, C, M>(
            context: &'a C,
            ctx: &'a CallCtx,
            raw: &'a str,
        ) -> BoxFuture<'a, Result<Value, Error>>
        where
            C: Handler<M> + Sync,
            M: Method,
        {
            Box::pin(async move { encode(context.handle(ctx, parse(raw)?).await?) })
        }

        Self {
            name: M::NAME,
            scope: M::SCOPE,
            dispatch: call::<C, M>,
        }
    }

    pub(crate) fn find<'a>(routes: &'a [Self], method: &str) -> Option<&'a Self> {
        routes.iter().find(|route| route.name == method)
    }

    /// Checks `ctx.scope` against the method's declared [`MethodScope`] before
    /// parsing params or invoking the handler.
    pub(crate) fn call<'a>(
        &self,
        service: &'a C,
        ctx: &'a CallCtx,
        raw: &'a str,
    ) -> BoxFuture<'a, Result<Value, Error>> {
        if let Err(error) = ctx.require(self.scope, self.name) {
            return Box::pin(async move { Err(error) });
        }
        (self.dispatch)(service, ctx, raw)
    }
}

pub(crate) fn parse<T: DeserializeOwned>(raw: &str) -> Result<T, Error> {
    serde_json::from_str(raw).map_err(|error| Error::invalid(error.to_string()))
}

pub(crate) fn unknown(method: &str) -> Error {
    RequestError::UnknownMethod(format!("unknown method {method}")).into()
}

/// Declare each procedure once, binding its canonical `viz_protocol::Method`
/// (and its declared [`MethodScope`]) directly to its typed implementation.
macro_rules! handlers {
    ($routes:ident for $context:ty { $($body:tt)+ }) => {
        crate::rpc::handlers!(@collect $routes, $context, [], $($body)+);
    };
    (@collect $routes:ident, $context:ty, [$($acc:ty),*],
        $method:ty, |$receiver:ident, $ctx:ident, $pattern:pat_param| $body:block $($rest:tt)*
    ) => {
        impl crate::rpc::Handler<$method> for $context {
            async fn handle(
                &self,
                $ctx: &crate::rpc::CallCtx,
                input: $method,
            ) -> Result<
                <$method as viz_protocol::Method>::Response,
                crate::error::Error,
            > {
                let $receiver = self;
                let $pattern = input;
                $body
            }
        }
        crate::rpc::handlers!(@collect $routes, $context, [$($acc,)* $method], $($rest)*);
    };
    (@collect $routes:ident, $context:ty, [$($acc:ty),*],
        $method:ty, |$receiver:ident, $pattern:pat_param| $body:block $($rest:tt)*
    ) => {
        impl crate::rpc::Handler<$method> for $context {
            async fn handle(
                &self,
                _ctx: &crate::rpc::CallCtx,
                input: $method,
            ) -> Result<
                <$method as viz_protocol::Method>::Response,
                crate::error::Error,
            > {
                let $receiver = self;
                let $pattern = input;
                $body
            }
        }
        crate::rpc::handlers!(@collect $routes, $context, [$($acc,)* $method], $($rest)*);
    };
    (@collect $routes:ident, $context:ty, [$($acc:ty),*],) => {
        pub(crate) const $routes: &[crate::rpc::Route<$context>] =
            &[$(crate::rpc::Route::new::<$acc>()),*];
    };
}
pub(crate) use handlers;
