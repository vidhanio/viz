//! Typed JSON-RPC dispatch. Only the transport adapter decodes input and
//! encodes results. Internal callers invoke the same handlers with Rust types.

use std::future::Future;

use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde_json::Value;
use viz_protocol::Method;

use crate::wire::{RpcError, encode};

pub(crate) trait Handler<M: Method> {
    type Input: DeserializeOwned + Send;

    fn handle(
        &self,
        input: Self::Input,
    ) -> impl Future<Output = Result<M::Response, RpcError>> + Send;
}

type Dispatch<C> = for<'a> fn(&'a C, &'a str) -> BoxFuture<'a, Result<Value, RpcError>>;

#[derive(Debug)]
pub(crate) struct Route<C> {
    name: &'static str,
    dispatch: Dispatch<C>,
}

impl<C: 'static> Route<C> {
    pub(crate) const fn new<M: Method>() -> Self
    where
        C: Handler<M> + Sync,
    {
        fn call<'a, C, M>(context: &'a C, raw: &'a str) -> BoxFuture<'a, Result<Value, RpcError>>
        where
            C: Handler<M> + Sync,
            M: Method,
        {
            Box::pin(async move { encode(context.handle(parse(raw)?).await?) })
        }

        Self {
            name: M::NAME,
            dispatch: call::<C, M>,
        }
    }

    pub(crate) fn find(routes: &[Self], method: &str) -> Option<Dispatch<C>> {
        routes
            .iter()
            .find(|route| route.name == method)
            .map(|route| route.dispatch)
    }
}

pub(crate) fn parse<T: DeserializeOwned>(raw: &str) -> Result<T, RpcError> {
    serde_json::from_str(raw).map_err(|error| RpcError::invalid(error.to_string()))
}

pub(crate) fn unknown(method: &str) -> RpcError {
    RpcError::new(-32601, format!("unknown method {method}"))
}

/// Declare each procedure once, with its transport input and typed
/// implementation.
macro_rules! handlers {
    ($routes:ident for $context:ty {
        $($method:ty, $input:ty, |$receiver:ident, $pattern:pat_param| $body:block)+
    }) => {
        pub(crate) const $routes: &[crate::rpc::Route<$context>] =
            &[$(crate::rpc::Route::new::<$method>()),+];

        $(
            impl crate::rpc::Handler<$method> for $context {
                type Input = $input;

                async fn handle(&self, input: Self::Input) -> Result<
                    <$method as viz_protocol::Method>::Response,
                    crate::wire::RpcError,
                > {
                    let $receiver = self;
                    let $pattern = input;
                    $body
                }
            }
        )+
    };
}
pub(crate) use handlers;
