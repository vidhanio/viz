//! Authenticated HTTP peer calls and incremental WebSocket sessions over
//! local Unix domain sockets and optional authenticated TCP listeners.

mod content;
mod peer_rpc;
mod socket;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Path as AxumPath, Request as HttpRequest, State, WebSocketUpgrade,
    },
    http::{
        HeaderMap, Method as HttpMethod, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE, ORIGIN},
    },
    middleware::{self, Next},
    response::{IntoResponse, Response as HttpResponse},
    routing::{get, put},
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, UnixListener};
use tokio_util::sync::CancellationToken;
use viz_protocol::{EnvironmentList, ListEnvironments, Method};

use crate::{
    config::Config,
    environments::Peers,
    error::{Error, RequestError},
    rpc::{self, CallCtx, Principal, Route, handlers},
    tasks::{self, Tasks},
    wire::{
        BLOBS_PATH, HEALTH_PATH, Hello, HelloParams, Id, PROTOCOL_VERSION, RPC_PATH, Request,
        Response, encode, params, raw_params,
    },
    workspace::{self, Workspace},
};

const MAX_FRAME: usize = viz_protocol::limits::MAX_WS_FRAME;
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) struct Server {
    workspace: Arc<Workspace>,
    peers: Arc<Peers>,
    tasks: Arc<Tasks>,
    environment: String,
    stop: CancellationToken,
}

#[derive(Clone)]
struct TcpAuth {
    client_token: Option<Arc<str>>,
    peer_tokens: Arc<[(Arc<str>, Arc<str>)]>,
}

impl TcpAuth {
    fn from_config(config: &Config) -> Self {
        let peer_tokens = config
            .environments
            .iter()
            .filter_map(|(id, peer)| {
                peer.accept
                    .as_ref()
                    .map(|token| (Arc::<str>::from(id.as_str()), Arc::clone(token)))
            })
            .collect();
        Self {
            client_token: config.token.clone(),
            peer_tokens,
        }
    }

    /// Evaluates every configured credential in constant time without
    /// short-circuiting so token comparisons do not leak slot order.
    fn authenticate(&self, headers: &HeaderMap) -> Option<Principal> {
        let given = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))?
            .as_bytes();
        let mut matched = self
            .client_token
            .as_ref()
            .filter(|token| bool::from(given.ct_eq(token.as_bytes())))
            .map(|_| Principal::Client);
        for (env_id, token) in self.peer_tokens.iter() {
            if bool::from(given.ct_eq(token.as_bytes())) {
                matched = Some(Principal::Peer(Arc::clone(env_id)));
            }
        }
        matched
    }
}

#[derive(Clone)]
enum Transport {
    Unix,
    Tcp(TcpAuth),
}

#[derive(Clone)]
struct Listener {
    server: Arc<Server>,
    transport: Transport,
}

impl Listener {
    fn router(self) -> Router {
        let guard_state = self.clone();
        Router::new()
            .route(HEALTH_PATH, get(health))
            .route(RPC_PATH, get(upgrade).post(peer_rpc))
            .route(BLOBS_PATH, put(peer_put_blob))
            .layer(middleware::from_fn_with_state(guard_state, guard))
            .layer(DefaultBodyLimit::max(MAX_FRAME))
            .with_state(self)
    }
}

async fn guard(
    State(listener): State<Listener>,
    mut request: HttpRequest,
    next: Next,
) -> HttpResponse {
    let headers = request.headers();
    if headers.contains_key(ORIGIN) {
        return (StatusCode::FORBIDDEN, "browser origin rejected").into_response();
    }
    if request.method() == HttpMethod::POST && !is_json_content_type(headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "expected Content-Type: application/json",
        )
            .into_response();
    }
    if headers
        .get("x-viz-environment")
        .is_some_and(|value| value.as_bytes() != listener.server.environment.as_bytes())
    {
        return (StatusCode::BAD_REQUEST, "environment identity mismatch").into_response();
    }
    let principal = match &listener.transport {
        Transport::Unix => Principal::Local,
        Transport::Tcp(auth) => match auth.authenticate(headers) {
            Some(principal) => principal,
            None => return StatusCode::UNAUTHORIZED.into_response(),
        },
    };
    request.extensions_mut().insert(principal);
    next.run(request).await
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
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
        unix: UnixListener,
        tcp: Option<TcpListener>,
    ) -> Result<()> {
        let unix_state = Listener {
            server: Arc::clone(self),
            transport: Transport::Unix,
        };
        let status = if let Some(tcp_listener) = &tcp {
            let addr = tcp_listener.local_addr()?;
            tracing::info!(
                environment = %self.environment,
                socket = %config.socket.display(),
                address = %addr,
                "viz listening"
            );
            format!("Listening on unix://{} and {addr}", config.socket.display())
        } else {
            tracing::info!(
                environment = %self.environment,
                socket = %config.socket.display(),
                "viz listening"
            );
            format!("Listening on unix://{}", config.socket.display())
        };
        crate::daemon::ready(&status)?;
        let serve_unix = async {
            axum::serve(unix, unix_state.router())
                .with_graceful_shutdown(self.stop.clone().cancelled_owned())
                .await
                .context("serving viz Unix RPC")
        };
        if let Some(tcp_listener) = tcp {
            let tcp_state = Listener {
                server: Arc::clone(self),
                transport: Transport::Tcp(TcpAuth::from_config(config)),
            };
            let serve_tcp = async {
                axum::serve(tcp_listener, tcp_state.router())
                    .with_graceful_shutdown(self.stop.clone().cancelled_owned())
                    .await
                    .context("serving viz TCP RPC")
            };
            tokio::try_join!(serve_unix, serve_tcp)?;
            Ok(())
        } else {
            serve_unix.await
        }
    }

    async fn call(&self, ctx: &CallCtx, method: &str, raw: &str) -> Result<Value, Error> {
        if let Some(route) = Route::find(ROUTES, method) {
            route.call(self, ctx, raw).await
        } else if let Some(route) = Route::find(tasks::ROUTES, method) {
            route.call(&self.tasks, ctx, raw).await
        } else if let Some(route) = Route::find(workspace::ROUTES, method) {
            route.call(&self.workspace, ctx, raw).await
        } else if let Some(route) = Route::find(peer_rpc::ROUTES, method) {
            route.call(self, ctx, raw).await
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

async fn health(
    State(listener): State<Listener>,
    Extension(_principal): Extension<Principal>,
) -> HttpResponse {
    Json(json!({
        "ok": true,
        "version": VERSION,
        "protocol": PROTOCOL_VERSION,
        "environment": listener.server.environment,
    }))
    .into_response()
}

async fn upgrade(
    State(listener): State<Listener>,
    Extension(principal): Extension<Principal>,
    ws: WebSocketUpgrade,
) -> HttpResponse {
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| async move {
            if let Err(error) = socket::connect(socket, listener.server, principal).await {
                tracing::debug!(%error, "client disconnected");
            }
        })
}

async fn peer_rpc(
    State(listener): State<Listener>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> HttpResponse {
    let Principal::Peer(source) = principal else {
        return (
            StatusCode::FORBIDDEN,
            "HTTP RPC is restricted to authenticated peers",
        )
            .into_response();
    };
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
    } else {
        let ctx = CallCtx::peer_http(source);
        let result = tokio::select! {
            () = listener.server.stop.cancelled() => Err(Error::unavailable("server is stopping")),
            result = listener.server.call(&ctx, request.method_name(), raw_params(&request)) => result,
        };
        match result {
            Ok(value) => Ok(value),
            Err(Error::Request(RequestError::Forbidden(message))) => {
                return (StatusCode::FORBIDDEN, message).into_response();
            }
            Err(Error::Request(RequestError::UnknownMethod(_))) => {
                return (
                    StatusCode::FORBIDDEN,
                    "method not permitted on peer HTTP RPC",
                )
                    .into_response();
            }
            Err(error) => Err(error),
        }
    };
    Json(Response::new(request.id, response)).into_response()
}

async fn peer_put_blob(
    State(listener): State<Listener>,
    Extension(principal): Extension<Principal>,
    AxumPath(sha): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> HttpResponse {
    let Principal::Peer(_) = principal else {
        return (
            StatusCode::FORBIDDEN,
            "blob upload is restricted to authenticated peers",
        )
            .into_response();
    };
    let octet_stream = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| {
            media
                .trim()
                .eq_ignore_ascii_case("application/octet-stream")
        });
    if !octet_stream {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "expected Content-Type: application/octet-stream",
        )
            .into_response();
    }
    if body.len() > viz_protocol::limits::ATTACHMENT_TOTAL_BYTES {
        return (StatusCode::PAYLOAD_TOO_LARGE, "blob exceeds 20 MiB limit").into_response();
    }
    match listener
        .server
        .workspace
        .receive_peer_blob(&sha, body.to_vec())
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            let status = match error.kind() {
                crate::error::Kind::Invalid => StatusCode::BAD_REQUEST,
                crate::error::Kind::NotFound => StatusCode::NOT_FOUND,
                crate::error::Kind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                crate::error::Kind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, error.to_string()).into_response()
        }
    }
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
