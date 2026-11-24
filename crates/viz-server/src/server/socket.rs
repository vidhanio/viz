//! WebSocket hello, connector replies, peer proxying, and incremental watches.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

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
    task::{AbortHandle, JoinSet},
    time::timeout,
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message as PeerMessage, client::IntoClientRequest, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;
use viz_protocol::{
    ConnectionChannel, Empty, ListEnvironments, Method, SendInput, TaskId, Unwatch, UnwatchGit,
    Watch, WatchGit, Watching, WorkspaceChange,
};

use super::{Server, parse_request};
use crate::{
    error::{Error, RequestError},
    rpc::{self, CallCtx, Principal},
    wire::{
        ClientKind, EntryId, Hello, HelloParams, Id, Notification, PROTOCOL_VERSION, Request,
        Response, RunStatus, ThreadId, ThreadState, params, raw_params,
    },
    workspace::{Change, EntityChange, WatchChange},
};

const PAGE: usize = 500;
const HANDSHAKE: Duration = Duration::from_secs(10);
const MAX_IN_FLIGHT: usize = 32;

pub(super) async fn connect(
    mut socket: WebSocket,
    server: Arc<Server>,
    principal: Principal,
) -> Result<()> {
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
        if matches!(principal, Principal::Peer(_)) {
            socket
                .send(json_message(&Response::<Value>::new(
                    request.id,
                    Err(Error::invalid(
                        "peer connections cannot proxy to another environment",
                    )),
                ))?)
                .await?;
            return Ok(());
        }
        return proxy(socket, &server, &environment, request.id, params, principal).await;
    }
    socket
        .send(json_message(&Response::new(request.id, Ok(hello)))?)
        .await?;
    let ctx = CallCtx::websocket(principal, params.client);
    match params.channel {
        ConnectionChannel::Control => local_session(socket, server, ctx).await,
        ConnectionChannel::Content => super::content::session(socket, server, ctx).await,
    }
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
    let (page, last, state) = server.workspace.watch_snapshot(thread, PAGE).await?;
    if app {
        server.workspace.watched(thread, WatchChange::Open).await?;
    }
    let _ = send_note(
        outgoing,
        Notification::ThreadView {
            thread,
            reset: true,
            entries: page.entries,
            before: page.before,
        },
    )
    .await;
    let _ = send_note(
        outgoing,
        Notification::ThreadState {
            thread,
            state: Box::new(state.clone()),
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
    loop {
        let (entries, state) = match server
            .workspace
            .watch_changes(thread, watch.last, PAGE)
            .await
        {
            Ok(changes) => changes,
            Err(Error::NoThread(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let last = entries.last().map(|entry| entry.id).or(watch.last);
        let has_entries = !entries.is_empty();
        if has_entries {
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
        }
        watch.last = last;
        let Some(state) = state else { continue };
        if state != watch.state {
            let mut previous = watch.state.clone();
            let previous_live = previous.live.take();
            let mut next = state.clone();
            let next_live = next.live.take();
            let note = if previous == next {
                let delta =
                    viz_protocol::LiveDelta::between(previous_live.as_ref(), next_live.as_ref());
                Notification::ThreadLive { thread, delta }
            } else {
                Notification::ThreadState {
                    thread,
                    state: Box::new(state.clone()),
                }
            };
            send_note(outgoing, note).await?;
        }
        watch.state = state;
        break;
    }
    Ok(())
}

async fn local_session(socket: WebSocket, server: Arc<Server>, ctx: CallCtx) -> Result<()> {
    let (write, mut read) = socket.split();
    let (outgoing, messages) = mpsc::channel::<Message>(32);
    let writer = tokio::spawn(write_frames(write, messages, server.stop.clone()));
    let mut workspace_events = server.workspace.subscribe();
    let mut calls = JoinSet::new();
    let mut watches = HashMap::new();
    let mut git_watches: HashMap<ThreadId, AbortHandle> = HashMap::new();
    let mut git_jobs = JoinSet::new();
    let app = ctx
        .client
        .as_ref()
        .is_some_and(|client| client.kind == ClientKind::App);
    let result = async {
        loop {
            tokio::select! {
                () = server.stop.cancelled() => break,
                () = outgoing.closed() => break,
                event = workspace_events.recv() => {
                    if !handle_workspace_event(event, &mut workspace_events, &server, &mut watches, &outgoing, app).await? {
                        break;
                    }
                },
                result = git_jobs.join_next_with_id(), if !git_jobs.is_empty() => {
                    if let Some(result) = result {
                        let id = result.map_or_else(|error| error.id(), |(id, ())| id);
                        git_watches.retain(|_, handle| handle.id() != id);
                    }
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
                            let result = match ctx.require(Watch::SCOPE, request.method_name()) {
                                Ok(()) => update_watch(&server, &request, &mut watches, &outgoing, app).await,
                                Err(error) => Err(error),
                            };
                            outgoing.send(json_message(&Response::new(request.id, result))?).await?;
                        }
                        WatchGit::NAME | UnwatchGit::NAME => {
                            let result = match ctx.require(WatchGit::SCOPE, request.method_name()) {
                                Ok(()) => update_git_watch(&server, &request, &mut git_watches, &mut git_jobs, &outgoing).await,
                                Err(error) => Err(error),
                            };
                            outgoing.send(json_message(&Response::new(request.id, result))?).await?;
                        }
                        HelloParams::NAME => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(Error::invalid("hello already sent"))))?).await?;
                        }
                        _ if calls.len() >= MAX_IN_FLIGHT => {
                            outgoing.send(json_message(&Response::<Value>::new(request.id, Err(Error::unavailable("too many requests in flight"))))?).await?;
                        }
                        _ => {
                            let args = raw_params(&request).to_owned();
                            let server = Arc::clone(&server);
                            let outgoing = outgoing.clone();
                            let ctx = ctx.clone();
                            let method = request.method.into_owned();
                            let id = request.id.into_owned();
                            calls.spawn(async move {
                                if let Err(error) = client_call(&server, &ctx, id, &method, &args, &outgoing).await {
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
    git_jobs.abort_all();
    writer.abort();
    close_watches(&server, watches, app).await;
    result
}

#[derive(Default)]
struct EventBatch {
    live_deltas: Vec<(ThreadId, Arc<viz_protocol::LiveDelta>)>,
    dirty_watches: HashSet<ThreadId>,
    harnesses_dirty: bool,
    entity_span: Option<(u64, u64)>,
    resync: bool,
    threads: HashMap<ThreadId, bool>,
    projects: HashMap<String, bool>,
    tasks: HashMap<TaskId, bool>,
}

impl EventBatch {
    fn push(&mut self, event: Change) {
        match event {
            Change::Live { thread, delta } => self.live_deltas.push((thread, delta)),
            Change::Thread(id) | Change::Settled(id) => {
                self.dirty_watches.insert(id);
            }
            Change::Harnesses => self.harnesses_dirty = true,
            Change::GitActivity => {}
            Change::Entity {
                base_rev,
                rev,
                change,
            } => {
                self.entity_span = Some(match self.entity_span {
                    Some((min_base, max_rev)) => (min_base.min(base_rev), max_rev.max(rev)),
                    None => (base_rev, rev),
                });
                match change {
                    EntityChange::ThreadUpsert(id) => {
                        self.threads.insert(id, true);
                    }
                    EntityChange::ThreadRemoved(id) => {
                        self.threads.insert(id, false);
                    }
                    EntityChange::ProjectUpsert(path) => {
                        self.projects.insert(path, true);
                    }
                    EntityChange::ProjectRemoved(path) => {
                        self.projects.insert(path, false);
                    }
                    EntityChange::TaskUpsert(id) => {
                        self.tasks.insert(id, true);
                    }
                    EntityChange::TaskRemoved(id) => {
                        self.tasks.insert(id, false);
                    }
                    EntityChange::Resync => self.resync = true,
                }
            }
        }
    }

    fn mark_lagged(&mut self, rev: u64, watched: impl Iterator<Item = ThreadId>) {
        self.dirty_watches.extend(watched);
        self.harnesses_dirty = true;
        self.resync = true;
        self.entity_span = Some(match self.entity_span {
            Some((min_base, max_rev)) => (min_base, max_rev.max(rev)),
            None => (rev, rev),
        });
    }
}

async fn handle_workspace_event(
    event: std::result::Result<Change, broadcast::error::RecvError>,
    workspace_events: &mut broadcast::Receiver<Change>,
    server: &Server,
    watches: &mut HashMap<ThreadId, WatchCursor>,
    outgoing: &mpsc::Sender<Message>,
    app: bool,
) -> Result<bool> {
    let mut batch = EventBatch::default();
    match event {
        Ok(change) => batch.push(change),
        Err(broadcast::error::RecvError::Lagged(_)) => {
            batch.mark_lagged(server.workspace.revision(), watches.keys().copied());
        }
        Err(broadcast::error::RecvError::Closed) => return Ok(false),
    }
    for _ in 0..256 {
        match workspace_events.try_recv() {
            Ok(change) => batch.push(change),
            Err(broadcast::error::TryRecvError::Empty) => break,
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                batch.mark_lagged(server.workspace.revision(), watches.keys().copied());
                break;
            }
            Err(broadcast::error::TryRecvError::Closed) => return Ok(false),
        }
    }
    for (thread, delta) in &batch.live_deltas {
        let thread = *thread;
        if batch.dirty_watches.contains(&thread) {
            continue;
        }
        if let Some(watch) = watches.get_mut(&thread) {
            let owned = delta.as_ref().clone();
            owned.clone().apply(&mut watch.state.live);
            send_note(
                outgoing,
                Notification::ThreadLive {
                    thread,
                    delta: owned,
                },
            )
            .await?;
        }
    }
    for &id in &batch.dirty_watches {
        refresh_watch(server, watches, id, outgoing).await?;
    }
    if app && let Some((base_rev, rev)) = batch.entity_span {
        let changes = resolve_entity_changes(server, &batch).await?;
        if !changes.is_empty() {
            send_note(
                outgoing,
                Notification::WorkspaceDelta {
                    base_rev,
                    rev,
                    changes,
                },
            )
            .await?;
        }
    }
    if app && batch.harnesses_dirty {
        send_note(outgoing, Notification::HarnessesChanged(Empty {})).await?;
    }
    Ok(true)
}

async fn resolve_entity_changes(
    server: &Server,
    batch: &EventBatch,
) -> Result<Vec<WorkspaceChange>> {
    if batch.resync {
        return Ok(vec![WorkspaceChange::Resync]);
    }
    let mut changes =
        Vec::with_capacity(batch.projects.len() + batch.threads.len() + batch.tasks.len());
    for (path, present) in &batch.projects {
        if *present {
            match server.workspace.read_project_opt(path).await? {
                Some(project) => changes.push(WorkspaceChange::ProjectUpsert {
                    project: Box::new(project),
                }),
                None => changes.push(WorkspaceChange::ProjectRemoved {
                    project: path.clone(),
                }),
            }
        } else {
            changes.push(WorkspaceChange::ProjectRemoved {
                project: path.clone(),
            });
        }
    }
    for (&thread, &present) in &batch.threads {
        if present {
            match server.workspace.read_thread_summary(thread).await? {
                Some(summary) => changes.push(WorkspaceChange::ThreadUpsert {
                    thread: Box::new(summary),
                }),
                None => changes.push(WorkspaceChange::ThreadRemoved { thread }),
            }
        } else {
            changes.push(WorkspaceChange::ThreadRemoved { thread });
        }
    }
    for (task_id, &present) in &batch.tasks {
        if present {
            match server.tasks.read_opt(task_id).await? {
                Some(task) => changes.push(WorkspaceChange::TaskUpsert {
                    task: Box::new(task),
                }),
                None => changes.push(WorkspaceChange::TaskRemoved {
                    task: task_id.clone(),
                }),
            }
        } else {
            changes.push(WorkspaceChange::TaskRemoved {
                task: task_id.clone(),
            });
        }
    }
    Ok(changes)
}

async fn close_watches(server: &Server, watches: HashMap<ThreadId, WatchCursor>, app: bool) {
    if app {
        for id in watches.into_keys() {
            if let Err(error) = server.workspace.watched(id, WatchChange::Close).await {
                tracing::debug!(thread = %id, %error, "clearing disconnected thread watch");
            }
        }
    }
}

/// The WebSocket owns the jobs, so closing it releases every checkout watch.
async fn update_git_watch(
    server: &Arc<Server>,
    request: &Request<'_>,
    watches: &mut HashMap<ThreadId, AbortHandle>,
    jobs: &mut JoinSet<()>,
    outgoing: &mpsc::Sender<Message>,
) -> Result<Value, Error> {
    if request.method_name() == UnwatchGit::NAME {
        let UnwatchGit { thread } = params(request)?;
        if let Some(handle) = watches.remove(&thread) {
            handle.abort();
        }
        return crate::wire::encode(Empty {});
    }
    let WatchGit { thread } = params(request)?;
    if watches.len() >= MAX_IN_FLIGHT && !watches.contains_key(&thread) {
        return Err(Error::unavailable("too many checkout subscriptions"));
    }
    let repo = server.workspace.git_repository(thread).await?;
    let status = server.workspace.git_status(&repo).await?;
    let handle = jobs.spawn(watch_checkout(
        Arc::clone(server),
        outgoing.clone(),
        thread,
        repo,
        status.clone(),
    ));
    if let Some(previous) = watches.insert(thread, handle) {
        previous.abort();
    }
    crate::wire::encode(status)
}

async fn watch_checkout(
    server: Arc<Server>,
    outgoing: mpsc::Sender<Message>,
    thread: ThreadId,
    mut repo: viz_git::Repository,
    initial: viz_protocol::GitStatus,
) {
    let cancel = server.stop.child_token();
    let mut changes = server.workspace.subscribe();
    let mut watch = repo.watch(Duration::from_millis(200), &cancel).await.ok();
    let mut timer = tokio::time::interval(Duration::from_secs(3));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = Some(initial);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = timer.tick(), if watch.is_none() => {},
            result = async { watch.as_mut().expect("watch present").changed(&cancel).await }, if watch.is_some() => {
                if result.is_err() {
                    watch = None;
                }
            },
            event = changes.recv() => {
                match event {
                    Ok(Change::GitActivity) => {
                        if !push_git_status(&server, &outgoing, thread, &repo, &mut previous).await {
                            break;
                        }
                        continue;
                    }
                    Ok(Change::Thread(id) | Change::Settled(id)) if id == thread => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => continue,
                }
                match server.workspace.git_repository(thread).await {
                    Ok(next) if next.identity() != repo.identity() => {
                        repo = next;
                        watch = repo.watch(Duration::from_millis(200), &cancel).await.ok();
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = send_note(&outgoing, Notification::GitChanged { thread, status: None }).await;
                        break;
                    }
                }
            }
        }
        if !push_git_status(&server, &outgoing, thread, &repo, &mut previous).await {
            break;
        }
    }
}

async fn push_git_status(
    server: &Server,
    outgoing: &mpsc::Sender<Message>,
    thread: ThreadId,
    repo: &viz_git::Repository,
    previous: &mut Option<viz_protocol::GitStatus>,
) -> bool {
    let status = server.workspace.git_status(repo).await.ok();
    if status == *previous {
        return true;
    }
    previous.clone_from(&status);
    send_note(outgoing, Notification::GitChanged { thread, status })
        .await
        .is_ok()
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
    ctx: &CallCtx,
    id: Id<'_>,
    method: &str,
    raw: &str,
    outgoing: &mpsc::Sender<Message>,
) -> Result<()> {
    let result = server.call(ctx, method, raw).await;
    let route_reply = if method == SendInput::NAME
        && let Some(connector) = ctx.connector()
        && let Ok(ref value) = result
        && let Ok(sent) = serde_json::from_value::<viz_protocol::Sent>(value.clone())
        && let Ok(input) = rpc::parse::<SendInput>(raw)
        && input.origin.is_some()
    {
        Some((input.thread, sent.run, connector.to_owned()))
    } else {
        None
    };
    outgoing
        .send(json_message(&Response::new(id, result))?)
        .await?;
    let Some((thread, run, connector)) = route_reply else {
        return Ok(());
    };
    let Some(origin) = server.workspace.run_origin(thread, run).await? else {
        return Ok(());
    };
    if origin.connector != connector {
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
        let (text, error) = match result.status {
            RunStatus::Completed => (result.answer, None),
            RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted => (
                None,
                Some(
                    result
                        .error
                        .unwrap_or_else(|| format!("run ended ({:?})", result.status)),
                ),
            ),
            RunStatus::Queued | RunStatus::Running => continue,
        };
        return send_note(
            outgoing,
            Notification::Reply {
                origin,
                thread,
                run,
                text,
                error,
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
    principal: Principal,
) -> Result<()> {
    let ctx = CallCtx::websocket(principal, params.client.clone());
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
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(viz_protocol::limits::MAX_WS_FRAME))
            .max_frame_size(Some(viz_protocol::limits::MAX_WS_FRAME));
        let (mut link, _) = timeout(
            HANDSHAKE,
            connect_async_with_config(request, Some(ws_config), false),
        )
        .await??;
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
    let channel = params.channel;
    server.peers.mark(environment, true);
    socket
        .send(json_message(&Response::new(id, Ok(response)))?)
        .await?;
    loop {
        tokio::select! {
            () = server.stop.cancelled() => break,
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break; };
                if !forward_proxy_frame(incoming?, channel, server, &ctx, &mut socket, &mut link).await? {
                    break;
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

async fn forward_proxy_frame<S>(
    incoming: Message,
    channel: ConnectionChannel,
    server: &Server,
    ctx: &CallCtx,
    socket: &mut WebSocket,
    link: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    match incoming {
        Message::Close(_) => return Ok(false),
        Message::Text(text) => {
            if channel == ConnectionChannel::Control
                && let Ok(request) = parse_request(text.as_bytes())
                && request.method == ListEnvironments::NAME
            {
                let result = server
                    .call(ctx, request.method_name(), raw_params(&request))
                    .await;
                socket
                    .send(json_message(&Response::new(request.id, result))?)
                    .await?;
            } else {
                link.send(PeerMessage::Text(text.as_str().into())).await?;
            }
        }
        Message::Binary(bytes) => {
            if channel == ConnectionChannel::Control
                && let Ok(request) = parse_request(&bytes)
                && request.method == ListEnvironments::NAME
            {
                let result = server
                    .call(ctx, request.method_name(), raw_params(&request))
                    .await;
                socket
                    .send(json_message(&Response::new(request.id, result))?)
                    .await?;
            } else {
                link.send(PeerMessage::Binary(bytes)).await?;
            }
        }
        Message::Ping(_) | Message::Pong(_) => {}
    }
    Ok(true)
}
