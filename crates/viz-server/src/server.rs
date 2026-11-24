//! Authenticated HTTP peer calls and incremental WebSocket sessions.

mod peer_rpc;
mod socket;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response as HttpResponse},
    routing::get,
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use viz_protocol::{
    EnvironmentList, ListEnvironments, Method, Unwatch, UnwatchGit, Watch, WatchGit,
};

use crate::{
    config::Config,
    environments::Peers,
    error::{Error, RequestError},
    rpc::{self, Route, handlers},
    tasks::{self, Tasks},
    wire::{
        HEALTH_PATH, Hello, HelloParams, Id, PROTOCOL_VERSION, RPC_PATH, Request, Response, encode,
        params, raw_params,
    },
    workspace::{self, Workspace},
};

const MAX_FRAME: usize = 32 * 1024 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) struct Server {
    workspace: Arc<Workspace>,
    peers: Arc<Peers>,
    tasks: Arc<Tasks>,
    environment: String,
    stop: CancellationToken,
}

#[derive(Clone)]
struct Listener {
    server: Arc<Server>,
    token: Option<Arc<str>>,
}

impl Listener {
    fn authorized(&self, headers: &HeaderMap) -> bool {
        self.token.as_ref().is_none_or(|token| {
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .is_some_and(|given| bool::from(given.as_bytes().ct_eq(token.as_bytes())))
        })
    }

    fn router(self) -> Router {
        Router::new()
            .route(HEALTH_PATH, get(health))
            .route(RPC_PATH, get(upgrade).post(peer_rpc))
            .layer(DefaultBodyLimit::max(MAX_FRAME))
            .with_state(self)
    }
}

impl Server {
    pub(crate) fn new(
        config: &Config,
        workspace: Arc<Workspace>,
        peers: Arc<Peers>,
        tasks: Arc<Tasks>,
        stop: CancellationToken,
    ) -> Self {
        Self {
            workspace,
            peers,
            tasks,
            environment: config.environment.clone(),
            stop,
        }
    }

    pub(crate) async fn run(
        self: &Arc<Self>,
        config: &Config,
        listener: TcpListener,
    ) -> Result<()> {
        let token = config
            .token
            .as_ref()
            .map(viz_config::Secret::resolve)
            .transpose()?
            .map(Arc::from);
        let state = Listener {
            server: Arc::clone(self),
            token,
        };
        tracing::info!(environment = %self.environment, address = %listener.local_addr()?, "viz listening");
        crate::daemon::ready(listener.local_addr()?)?;
        axum::serve(listener, state.router())
            .with_graceful_shutdown(self.stop.clone().cancelled_owned())
            .await
            .context("serving viz RPC")
    }

    async fn call(&self, method: &str, raw: &str) -> Result<Value, Error> {
        if let Some(call) = Route::find(ROUTES, method) {
            call(self, raw).await
        } else if let Some(call) = Route::find(tasks::ROUTES, method) {
            call(&self.tasks, raw).await
        } else if let Some(call) = Route::find(workspace::ROUTES, method) {
            call(&self.workspace, raw).await
        } else {
            Err(rpc::unknown(method))
        }
    }

    fn hello(&self, params: &HelloParams) -> Result<Hello, Error> {
        if params.protocol != PROTOCOL_VERSION {
            return Err(Error::invalid(format!(
                "expected protocol {PROTOCOL_VERSION}"
            )));
        }
        Ok(Hello {
            protocol: PROTOCOL_VERSION,
            version: VERSION.into(),
            environment: self.environment.clone(),
        })
    }
}

handlers! {
    ROUTES for Server {
        ListEnvironments, |server, _| {
            Ok(EnvironmentList { environments: server.peers.list().await })
        }
    }
}

async fn health(State(listener): State<Listener>, headers: HeaderMap) -> HttpResponse {
    if !listener.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "ok": true, "version": VERSION, "protocol": PROTOCOL_VERSION, "environment": listener.server.environment })).into_response()
}

async fn upgrade(
    State(listener): State<Listener>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> HttpResponse {
    if !listener.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if headers
        .get("x-viz-environment")
        .is_some_and(|value| value.as_bytes() != listener.server.environment.as_bytes())
    {
        return (StatusCode::BAD_REQUEST, "environment identity mismatch").into_response();
    }
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| async move {
            if let Err(error) = socket::connect(socket, listener.server).await {
                tracing::debug!(%error, "client disconnected");
            }
        })
}

async fn peer_rpc(
    State(listener): State<Listener>,
    headers: HeaderMap,
    body: Bytes,
) -> HttpResponse {
    if !listener.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if headers
        .get("x-viz-environment")
        .is_some_and(|value| value.as_bytes() != listener.server.environment.as_bytes())
    {
        return (StatusCode::BAD_REQUEST, "environment identity mismatch").into_response();
    }
    let request = match parse_request(&body) {
        Ok(request) => request,
        Err(error) => {
            return Json(Response::<Value>::new(Id::Null, Err(error.into()))).into_response();
        }
    };
    let response = if request.method == HelloParams::NAME {
        params::<HelloParams>(&request).and_then(|params| {
            if params
                .environment
                .as_deref()
                .is_some_and(|id| id != listener.server.environment)
            {
                return Err(Error::invalid(
                    "peer RPC must address the receiving environment",
                ));
            }
            encode(listener.server.hello(&params)?)
        })
    } else if let Some(dispatch) = peer_rpc::find(&request.method) {
        if listener.token.is_none() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let Some(source) = headers
            .get("x-viz-source")
            .and_then(|value| value.to_str().ok())
            .filter(|id| listener.server.peers.get(id).is_some())
        else {
            return (StatusCode::FORBIDDEN, "unconfigured source environment").into_response();
        };
        let result = tokio::select! {
            () = listener.server.stop.cancelled() => Err(Error::unavailable("server is stopping").into()),
            result = dispatch(&listener.server, raw_params(&request), source) => result,
        };
        match result {
            Ok(value) => Ok(value),
            Err(peer_rpc::PeerError::Rpc(error)) => Err(error),
            Err(peer_rpc::PeerError::SourceMismatch) => {
                return (StatusCode::FORBIDDEN, "source environment mismatch").into_response();
            }
        }
    } else if matches!(
        request.method_name(),
        Watch::NAME | Unwatch::NAME | WatchGit::NAME | UnwatchGit::NAME
    ) {
        Err(RequestError::UnknownMethod("watch requires a WebSocket".into()).into())
    } else {
        tokio::select! {
            () = listener.server.stop.cancelled() => Err(Error::unavailable("server is stopping")),
            result = listener.server.call(request.method_name(), raw_params(&request)) => result,
        }
    };
    Json(Response::new(request.id, response)).into_response()
}

fn parse_request(data: &[u8]) -> Result<Request<'_>, RequestError> {
    let request: Request<'_> = serde_json::from_slice(data).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            RequestError::Malformed(error.to_string())
        } else {
            RequestError::NotARequest(error.to_string())
        }
    })?;
    if request.id == Id::Null {
        return Err(RequestError::NotARequest(
            "expected a JSON-RPC 2.0 request with an ID".into(),
        ));
    }
    Ok(request)
}
