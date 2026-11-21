//! Authenticated HTTP peer calls and incremental WebSocket sessions.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response as HttpResponse},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    task::JoinSet,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as PeerMessage, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use viz_client::protocol::{Empty, ListEnvironments, Method, SendInput, Unwatch, Watch, Watching};

use crate::{
    config::Config,
    environments::Peers,
    tasks::Tasks,
    wire::{
        self, ClientKind, HEALTH_PATH, Hello, HelloParams, Id, Notification, PROTOCOL_VERSION,
        RPC_PATH, Request, Response, RpcError, RunStatus, ThreadId, ThreadState, decode, encode,
        params,
    },
    workspace::{Change, WatchChange, Workspace},
};

const MAX_FRAME: usize = 32 * 1024 * 1024;
const PAGE: usize = 100;
const HANDSHAKE: Duration = Duration::from_secs(10);
const MAX_IN_FLIGHT: usize = 32;
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

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "environments.list" => encode(viz_client::protocol::EnvironmentList {
                environments: self.peers.list().await,
            }),
            "tasks.list" | "task.delegate" | "task.status" | "task.cancel" | "task.progress"
            | "task.follow_up" => self.tasks.call(method, params).await,
            _ => self.workspace.call(method, params).await,
        }
    }

    fn hello(&self, params: &HelloParams) -> Result<Hello, RpcError> {
        if params.protocol != PROTOCOL_VERSION {
            return Err(RpcError::invalid(format!(
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
            if let Err(error) = connect(socket, listener.server).await {
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
        Err(error) => return Json(Response::<Value>::new(Id::Null, Err(error))).into_response(),
    };
    let response = if request.method == HelloParams::NAME {
        params::<HelloParams>(&request).and_then(|params| {
            if params
                .environment
                .as_deref()
                .is_some_and(|id| id != listener.server.environment)
            {
                return Err(RpcError::invalid(
                    "peer RPC must address the receiving environment",
                ));
            }
            encode(listener.server.hello(&params)?)
        })
    } else if internal(&request.method) {
        if listener.token.is_none() {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let source = headers
            .get("x-viz-source")
            .and_then(|value| value.to_str().ok());
        if source.is_none_or(|id| listener.server.peers.get(id).is_none()) {
            return (StatusCode::FORBIDDEN, "unconfigured source environment").into_response();
        }
        let args = match params::<Value>(&request) {
            Ok(args) => args,
            Err(error) => {
                return Json(Response::<Value>::new(request.id, Err(error))).into_response();
            }
        };
        if matches!(request.method_name(), "task.receive" | "task.retract")
            && args.get("originEnvironment").and_then(Value::as_str) != source
        {
            return (StatusCode::FORBIDDEN, "source environment mismatch").into_response();
        }
        if request.method == "task.follow_up"
            && args.get("source").and_then(Value::as_str) != source
        {
            return (StatusCode::FORBIDDEN, "source environment mismatch").into_response();
        }
        http_call(&listener.server, request.method_name(), args).await
    } else if matches!(request.method_name(), Watch::NAME | Unwatch::NAME) {
        Err(RpcError::new(-32601, "watch requires a WebSocket"))
    } else {
        match params(&request) {
            Ok(args) => http_call(&listener.server, request.method_name(), args).await,
            Err(error) => Err(error),
        }
    };
    Json(Response::new(request.id, response)).into_response()
}

async fn http_call(server: &Server, method: &str, args: Value) -> Result<Value, RpcError> {
    tokio::select! {
        () = server.stop.cancelled() => Err(RpcError::unavailable("server is stopping")),
        result = server.call(method, args) => result,
    }
}

fn internal(method: &str) -> bool {
    matches!(
        method,
        "task.receive" | "task.progress" | "task.retract" | "task.follow_up"
    )
}

fn parse_request(data: &[u8]) -> Result<Request<'_>, RpcError> {
    let request: Request<'_> = serde_json::from_slice(data).map_err(|error| {
        RpcError::new(
            if error.is_syntax() || error.is_eof() {
                -32700
            } else {
                -32600
            },
            error.to_string(),
        )
    })?;
    if request.id == Id::Null {
        return Err(RpcError::new(
            -32600,
            "expected a JSON-RPC 2.0 request with an ID",
        ));
    }
    Ok(request)
}

async fn connect(mut socket: WebSocket, server: Arc<Server>) -> Result<()> {
    let message = timeout(HANDSHAKE, socket.next())
        .await?
        .context("closed before hello")??
        .into_data();
    let request = match parse_request(&message) {
        Ok(request) if request.method == HelloParams::NAME => request,
        Ok(request) => {
            socket
                .send(json_message(&Response::<Value>::new(
                    request.id,
                    Err(RpcError::new(-32000, "call hello first")),
                ))?)
                .await?;
            return Ok(());
        }
        Err(error) => {
            socket
                .send(json_message(&Response::<Value>::new(Id::Null, Err(error)))?)
                .await?;
            return Ok(());
        }
    };
    let params = match params::<HelloParams>(&request)
        .and_then(|params| server.hello(&params).map(|hello| (params, hello)))
    {
        Ok(pair) => pair,
        Err(error) => {
            socket
                .send(json_message(&Response::<Value>::new(
                    request.id,
                    Err(error),
                ))?)
                .await?;
            return Ok(());
        }
    };
    let (mut params, hello) = params;
    if let Some(environment) = params
        .environment
        .take()
        .filter(|id| *id != server.environment)
    {
        return proxy(socket, &server, &environment, request.id, params).await;
    }
    socket
        .send(json_message(&Response::new(request.id, Ok(hello)))?)
        .await?;
    local_session(socket, server, params.client).await
}

#[derive(Serialize)]
struct NotificationFrame {
    jsonrpc: &'static str,
    #[serde(flatten)]
    notification: Notification,
}

fn notification(value: Notification) -> serde_json::Result<Message> {
    json_message(&NotificationFrame {
        jsonrpc: "2.0",
        notification: value,
    })
}

fn json_message(value: &impl Serialize) -> serde_json::Result<Message> {
    serde_json::to_string(value).map(|text| Message::Text(text.into()))
}

struct WatchCursor {
    last: u64,
    state: ThreadState,
}

async fn watch_thread(
    server: &Server,
    thread: ThreadId,
    outgoing: &mpsc::Sender<Message>,
    app: bool,
) -> Result<WatchCursor, RpcError> {
    let page = server.workspace.entries(thread.0, None, PAGE).await?;
    let state = server.workspace.state(thread.0).await?;
    let last = page.entries.last().map_or(0, |entry| entry.id.0);
    if app {
        server
            .workspace
            .watched(thread.0, WatchChange::Open)
            .await?;
    }
    let view = Notification::ThreadView {
        thread,
        reset: true,
        entries: page.entries,
        before: page.before,
    };
    let _ = send_note(outgoing, view).await;
    let _ = send_note(
        outgoing,
        Notification::ThreadState {
            thread,
            state: state.clone(),
        },
    )
    .await;
    Ok(WatchCursor { last, state })
}

async fn send_note(outgoing: &mpsc::Sender<Message>, value: Notification) -> Result<()> {
    outgoing
        .send(notification(value)?)
        .await
        .context("client disconnected")
}

async fn refresh_watch(
    server: &Server,
    watches: &mut HashMap<ThreadId, WatchCursor>,
    thread: ThreadId,
    outgoing: &mpsc::Sender<Message>,
) -> Result<()> {
    let Some(watch) = watches.get_mut(&thread) else {
        return Ok(());
    };
    let state = match server.workspace.state(thread.0).await {
        Ok(state) => state,
        Err(error) if error.code == wire::NOT_FOUND => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if watch.state.history_revision != state.history_revision {
        let page = server.workspace.entries(thread.0, None, PAGE).await?;
        watch.last = page.entries.last().map_or(0, |entry| entry.id.0);
        send_note(
            outgoing,
            Notification::ThreadView {
                thread,
                reset: true,
                entries: page.entries,
                before: page.before,
            },
        )
        .await?;
    }
    loop {
        let entries = server
            .workspace
            .entries_after(thread.0, watch.last, PAGE)
            .await?;
        let Some(last) = entries.last().map(|entry| entry.id.0) else {
            break;
        };
        let more = entries.len() == PAGE;
        send_note(
            outgoing,
            Notification::ThreadView {
                thread,
                entries,
                reset: false,
                before: None,
            },
        )
        .await?;
        watch.last = last;
        if !more {
            break;
        }
    }
    if watch.state != state {
        send_note(
            outgoing,
            Notification::ThreadState {
                thread,
                state: state.clone(),
            },
        )
        .await?;
        watch.state = state;
    }
    Ok(())
}

async fn local_session(
    socket: WebSocket,
    server: Arc<Server>,
    client: viz_client::protocol::ClientInfo,
) -> Result<()> {
    let (write, mut read) = socket.split();
    let (outgoing, messages) = mpsc::channel::<Message>(32);
    let writer = tokio::spawn(write_frames(write, messages, server.stop.clone()));
    let mut workspace_events = server.workspace.subscribe();
    let mut tasks = server.tasks.subscribe();
    let mut calls = JoinSet::new();
    let mut watches = HashMap::new();
    let app = client.kind == ClientKind::App;
    let result = async {
        loop {
            tokio::select! {
                () = server.stop.cancelled() => break,
                () = outgoing.closed() => break,
                event = workspace_events.recv() => match event {
                    Ok(Change::Thread(id) | Change::Settled(id)) => {
                        refresh_watch(&server, &mut watches, ThreadId(id), &outgoing).await?;
                    }
                    Ok(Change::Directory) => if app {
                        send_note(&outgoing, Notification::ThreadsChanged(Empty {})).await?;
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let ids: Vec<_> = watches.keys().copied().collect();
                        for id in ids { refresh_watch(&server, &mut watches, id, &outgoing).await?; }
                        if app { send_note(&outgoing, Notification::ThreadsChanged(Empty {})).await?; }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                change = tasks.recv(), if app => match change {
                    Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => send_note(&outgoing, Notification::TasksChanged(Empty {})).await?,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = calls.join_next(), if !calls.is_empty() => {},
                frame = read.next() => {
                    let Some(frame) = frame else { break; };
                    let frame = frame?;
                    if matches!(frame, Message::Close(_)) { break; }
                    if !matches!(frame, Message::Text(_) | Message::Binary(_)) { continue; }
                    let frame = frame.into_data();
                    let request = match parse_request(&frame) {
                        Ok(request) => request,
                        Err(error) => {
                            outgoing.send(json_message(&Response::<Value>::new(Id::Null, Err(error)))?).await?;
                            continue;
                        }
                    };
                    match request.method_name() {
                        Watch::NAME | Unwatch::NAME => {
                            let result = update_watch(&server, &request, &mut watches, &outgoing, app).await;
                            outgoing.send(json_message(&Response::new(request.id, result))?).await?;
                        }
                        HelloParams::NAME => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(RpcError::invalid("hello already sent"))))?).await?;
                        }
                        method if internal(method) => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(RpcError::new(-32601, "internal peer method"))))?).await?;
                        }
                        _ if calls.len() >= MAX_IN_FLIGHT => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(RpcError::unavailable("too many requests in flight"))))?).await?;
                        }
                        _ => {
                            let args = match params(&request) {
                                Ok(args) => args,
                                Err(error) => {
                                    outgoing.send(json_message(&Response::<Value>::new(request.id, Err(error)))?).await?;
                                    continue;
                                }
                            };
                            let server = Arc::clone(&server);
                            let outgoing = outgoing.clone();
                            let connector = (client.kind == ClientKind::Connector).then(|| client.name.clone());
                            let method = request.method.into_owned();
                            let id = request.id.into_owned();
                            calls.spawn(async move {
                                if let Err(error) = client_call(&server, id, &method, args, &outgoing, connector.as_deref()).await {
                                    tracing::debug!(%error, %method, "client request ended");
                                }
                            });
                        }
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }.await;
    calls.abort_all();
    writer.abort();
    if app {
        for id in watches.into_keys() {
            if let Err(error) = server.workspace.watched(id.0, WatchChange::Close).await {
                tracing::debug!(thread = %id, %error, "clearing disconnected thread watch");
            }
        }
    }
    result
}

async fn write_frames(
    mut write: futures_util::stream::SplitSink<WebSocket, Message>,
    mut messages: mpsc::Receiver<Message>,
    stop: CancellationToken,
) {
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            frame = messages.recv() => {
                let Some(frame) = frame else { break; };
                if write.send(frame).await.is_err() { break; }
            }
        }
    }
}

async fn update_watch(
    server: &Server,
    request: &Request<'_>,
    watches: &mut HashMap<ThreadId, WatchCursor>,
    outgoing: &mpsc::Sender<Message>,
    app: bool,
) -> Result<Watching, RpcError> {
    if request.method == Watch::NAME {
        let Watch { thread } = params(request)?;
        if let std::collections::hash_map::Entry::Vacant(slot) = watches.entry(thread) {
            slot.insert(watch_thread(server, thread, outgoing, app).await?);
        }
        Ok(Watching { watching: true })
    } else {
        let Unwatch { thread } = params(request)?;
        if watches.remove(&thread).is_some() && app {
            server
                .workspace
                .watched(thread.0, WatchChange::Close)
                .await?;
        }
        Ok(Watching { watching: false })
    }
}

async fn client_call(
    server: &Server,
    id: Id<'_>,
    method: &str,
    args: Value,
    outgoing: &mpsc::Sender<Message>,
    connector: Option<&str>,
) -> Result<()> {
    if method != SendInput::NAME {
        let result = server.call(method, args).await;
        outgoing
            .send(json_message(&Response::new(id, result))?)
            .await?;
        return Ok(());
    }
    let input = match decode::<SendInput>(args) {
        Ok(input)
            if input
                .origin
                .as_ref()
                .is_none_or(|origin| Some(origin.connector.as_str()) == connector) =>
        {
            input
        }
        Ok(_) => {
            outgoing
                .send(json_message(&Response::<Value>::new(
                    id,
                    Err(RpcError::invalid(
                        "origin must belong to the connected connector",
                    )),
                ))?)
                .await?;
            return Ok(());
        }
        Err(error) => {
            outgoing
                .send(json_message(&Response::<Value>::new(id, Err(error)))?)
                .await?;
            return Ok(());
        }
    };
    let thread = input.thread;
    let route = input.origin.is_some();
    let sent = server.workspace.send(input).await;
    let run = sent.as_ref().ok().map(|sent| sent.run);
    outgoing
        .send(json_message(&Response::new(id, sent))?)
        .await?;
    let Some(run) = run.filter(|_| route) else {
        return Ok(());
    };
    let Some(origin) = server.workspace.run_origin(thread.0, run.0).await? else {
        return Ok(());
    };
    if Some(origin.connector.as_str()) != connector {
        return Ok(());
    }
    loop {
        let waiting = server
            .workspace
            .wait_run(thread.0, Some(run.0), Duration::from_secs(30));
        let status = tokio::select! {
            () = outgoing.closed() => return Ok(()),
            () = server.stop.cancelled() => return Ok(()),
            result = waiting => result?,
        };
        let Some(result) = status.run else {
            return Ok(());
        };
        if matches!(result.status, RunStatus::Queued | RunStatus::Running) {
            continue;
        }
        return send_note(
            outgoing,
            Notification::Reply {
                origin,
                thread,
                run,
                text: result.answer,
                error: result.error,
            },
        )
        .await;
    }
}

async fn proxy(
    mut socket: WebSocket,
    server: &Server,
    environment: &str,
    id: Id<'_>,
    params: HelloParams,
) -> Result<()> {
    let connected = async {
        let peer = server
            .peers
            .get(environment)
            .context("unknown environment")?;
        let mut url = peer.url.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|()| anyhow::anyhow!("invalid peer scheme"))?;
        let mut request = url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert(AUTHORIZATION, format!("Bearer {}", peer.token).parse()?);
        request
            .headers_mut()
            .insert("x-viz-environment", environment.parse()?);
        let (mut link, _) = timeout(HANDSHAKE, connect_async(request)).await??;
        link.send(PeerMessage::Text(
            serde_json::to_string(
                &json!({"jsonrpc":"2.0","id":id,"method":"hello","params":params}),
            )?
            .into(),
        ))
        .await?;
        let response = timeout(HANDSHAKE, link.next())
            .await?
            .context("peer closed before hello")??;
        let bytes = response.into_data();
        let result: jsonrpsee_types::Response<'_, Hello> = serde_json::from_slice(&bytes)?;
        ensure!(result.id == id, "peer returned the wrong request ID");
        let reply = jsonrpsee_types::ResponseSuccess::try_from(result)
            .map_err(|error| anyhow::anyhow!("peer rejected hello: {error}"))?;
        ensure!(
            reply.result.protocol == PROTOCOL_VERSION && reply.result.environment == environment,
            "peer identity or protocol changed"
        );
        Ok::<_, anyhow::Error>((link, reply.result))
    }
    .await;
    let (mut link, response) = match connected {
        Ok(pair) => pair,
        Err(error) => {
            server.peers.mark(environment, false);
            socket
                .send(json_message(&Response::<Value>::new(
                    id,
                    Err(RpcError::unavailable(error.to_string())),
                ))?)
                .await?;
            return Ok(());
        }
    };
    server.peers.mark(environment, true);
    socket
        .send(json_message(&Response::new(id, Ok(response)))?)
        .await?;
    loop {
        tokio::select! {
            () = server.stop.cancelled() => break,
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break; };
                let incoming = incoming?;
                if matches!(incoming, Message::Close(_)) { break; }
                if !matches!(incoming, Message::Text(_) | Message::Binary(_)) { continue; }
                let bytes = incoming.into_data();
                if let Ok(request) = parse_request(&bytes) && request.method == ListEnvironments::NAME {
                    let result = match wire::params(&request) {
                        Ok(args) => server.call(request.method_name(), args).await,
                        Err(error) => Err(error),
                    };
                    socket.send(json_message(&Response::new(request.id, result))?).await?;
                } else {
                    link.send(PeerMessage::Binary(bytes)).await?;
                }
            }
            incoming = link.next() => match incoming {
                Some(Ok(PeerMessage::Text(text))) => socket.send(Message::Text(text.as_str().into())).await?,
                Some(Ok(PeerMessage::Binary(bytes))) => socket.send(Message::Binary(bytes)).await?,
                Some(Ok(PeerMessage::Close(_)) | Err(_)) | None => break,
                _ => {},
            },
        }
    }
    let _ = link.close(None).await;
    Ok(())
}
