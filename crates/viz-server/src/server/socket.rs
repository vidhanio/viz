//! WebSocket hello, connector replies, peer proxying, and incremental watches.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use axum::{
    extract::ws::{Message, WebSocket},
    http::header::AUTHORIZATION,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinSet,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as PeerMessage, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use viz_protocol::{Empty, ListEnvironments, Method, SendInput, Unwatch, Watch, Watching};

use super::{Server, parse_request, peer_rpc};
use crate::{
    error::{Error, RequestError},
    rpc,
    wire::{
        ClientKind, EntryId, Hello, HelloParams, Id, Notification, PROTOCOL_VERSION, Request,
        Response, RunStatus, ThreadId, ThreadState, params, raw_params,
    },
    workspace::{Change, WatchChange},
};

const PAGE: usize = 100;
const HANDSHAKE: Duration = Duration::from_secs(10);
const MAX_IN_FLIGHT: usize = 32;

pub(super) async fn connect(mut socket: WebSocket, server: Arc<Server>) -> Result<()> {
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
                    Err(RequestError::HelloRequired.into()),
                ))?)
                .await?;
            return Ok(());
        }
        Err(error) => {
            socket
                .send(json_message(&Response::<Value>::new(
                    Id::Null,
                    Err(error.into()),
                ))?)
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
    last: Option<EntryId>,
    state: ThreadState,
}

async fn watch_thread(
    server: &Server,
    thread: ThreadId,
    outgoing: &mpsc::Sender<Message>,
    app: bool,
) -> Result<WatchCursor, Error> {
    let page = server.workspace.entries(thread, None, PAGE).await?;
    let state = server.workspace.state(thread).await?;
    let last = page.entries.last().map(|entry| entry.id);
    if app {
        server.workspace.watched(thread, WatchChange::Open).await?;
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
    let state = match server.workspace.state(thread).await {
        Ok(state) => state,
        Err(Error::NoThread(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    loop {
        let entries = server
            .workspace
            .entries_after(thread, watch.last, PAGE)
            .await?;
        let Some(last) = entries.last().map(|entry| entry.id) else {
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
        watch.last = Some(last);
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
    client: viz_protocol::ClientInfo,
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
                        refresh_watch(&server, &mut watches, id, &outgoing).await?;
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
                            outgoing.send(json_message(&Response::<Value>::new(Id::Null, Err(error.into())))?).await?;
                            continue;
                        }
                    };
                    match request.method_name() {
                        Watch::NAME | Unwatch::NAME => {
                            let result = update_watch(&server, &request, &mut watches, &outgoing, app).await;
                            outgoing.send(json_message(&Response::new(request.id, result))?).await?;
                        }
                        HelloParams::NAME => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(Error::invalid("hello already sent"))))?).await?;
                        }
                        method if peer_rpc::find(method).is_some() => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(RequestError::UnknownMethod("internal peer method".into()).into())))?).await?;
                        }
                        _ if calls.len() >= MAX_IN_FLIGHT => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(Error::unavailable("too many requests in flight"))))?).await?;
                        }
                        _ => {
                            let args = raw_params(&request).to_owned();
                            let server = Arc::clone(&server);
                            let outgoing = outgoing.clone();
                            let connector = (client.kind == ClientKind::Connector).then(|| client.name.clone());
                            let method = request.method.into_owned();
                            let id = request.id.into_owned();
                            calls.spawn(async move {
                                if let Err(error) = client_call(&server, id, &method, &args, &outgoing, connector.as_deref()).await {
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
            if let Err(error) = server.workspace.watched(id, WatchChange::Close).await {
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
) -> Result<Watching, Error> {
    if request.method == Watch::NAME {
        let Watch { thread } = params(request)?;
        if let std::collections::hash_map::Entry::Vacant(slot) = watches.entry(thread) {
            slot.insert(watch_thread(server, thread, outgoing, app).await?);
        }
        Ok(Watching { watching: true })
    } else {
        let Unwatch { thread } = params(request)?;
        if watches.remove(&thread).is_some() && app {
            server.workspace.watched(thread, WatchChange::Close).await?;
        }
        Ok(Watching { watching: false })
    }
}

async fn client_call(
    server: &Server,
    id: Id<'_>,
    method: &str,
    raw: &str,
    outgoing: &mpsc::Sender<Message>,
    connector: Option<&str>,
) -> Result<()> {
    if method != SendInput::NAME {
        let result = server.call(method, raw).await;
        outgoing
            .send(json_message(&Response::new(id, result))?)
            .await?;
        return Ok(());
    }
    let input = match rpc::parse::<SendInput>(raw) {
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
                    Err(Error::invalid(
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
    let Some(origin) = server.workspace.run_origin(thread, run).await? else {
        return Ok(());
    };
    if Some(origin.connector.as_str()) != connector {
        return Ok(());
    }
    loop {
        let waiting = server
            .workspace
            .wait_run(thread, Some(run), Duration::from_secs(30));
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
                    Err(Error::unavailable(error.to_string())),
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
                    let result = server.call(request.method_name(), raw_params(&request)).await;
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
