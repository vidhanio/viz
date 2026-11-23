//! Supervised ACP v1 over newline-delimited JSON-RPC.
//!
//! The SDK's incoming actor awaits dispatch before routing the next frame; its
//! typed schema and raw envelopes are used here. The 5.5 process launcher
//! inherits the environment and has no working-directory setting. This
//! transport starts the child with a scrubbed environment and explicit cwd,
//! drains queued writes before EOF, and reaps the process after a grace period.
//! A synchronous response hook installs the prompt boundary before the next
//! update can reach a session. Dropping a pending request sends the typed
//! `$/cancel_request` notification; cancelling a turn sends `session/cancel`.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use agent_client_protocol::{
    JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RawJsonRpcId,
    RawJsonRpcMessage, RawJsonRpcParams, RawJsonRpcRequest, RawJsonRpcResponse,
};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ClientEvent, Error,
    error::Result,
    interaction::Incoming,
    process::EXIT_GRACE,
    schema::{AgentNotification, CancelRequestNotification, RequestId, SessionNotification},
};

/// Receives each session update on the reader before the following frame.
pub type UpdateSink = Arc<dyn Fn(SessionNotification, i64) + Send + Sync>;
pub type ResponseHook = Box<dyn FnOnce() + Send>;

struct Waiter {
    method: String,
    response: oneshot::Sender<Result<Value>>,
    on_response: Option<ResponseHook>,
}

pub struct Connection {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<i64, Waiter>>,
    next_id: AtomicI64,
    pub closed: CancellationToken,
    reason: Mutex<Option<String>>,
    pub incoming: Arc<Incoming>,
}

impl Connection {
    pub fn new(outgoing: mpsc::UnboundedSender<String>, incoming: Incoming) -> Self {
        Self {
            outgoing,
            pending: Mutex::default(),
            next_id: AtomicI64::new(1),
            closed: CancellationToken::new(),
            reason: Mutex::default(),
            incoming: Arc::new(incoming),
        }
    }

    fn send(&self, message: &RawJsonRpcMessage) -> Result<()> {
        self.outgoing
            .send(serde_json::to_string(message)?)
            .map_err(|_| self.closed_error())
    }

    fn close(&self, reason: String) {
        self.reason
            .lock()
            .expect("ACP close reason lock poisoned")
            .get_or_insert(reason);
        self.closed.cancel();
        let waiters = std::mem::take(&mut *self.pending.lock().expect("ACP pending lock poisoned"));
        for (_, waiter) in waiters {
            let _ = waiter.response.send(Err(self.closed_error()));
        }
    }

    pub fn reason(&self) -> String {
        self.reason
            .lock()
            .expect("ACP close reason lock poisoned")
            .clone()
            .unwrap_or_else(|| "the harness closed its input".to_owned())
    }

    fn closed_error(&self) -> Error {
        Error::Closed(self.reason())
    }

    pub fn notify(&self, notification: &impl JsonRpcNotification) -> Result<()> {
        let message = notification
            .to_untyped_message()
            .map_err(|error| Error::protocol(notification.method(), error))?;
        self.send(
            &RawJsonRpcMessage::notification(message.method, message.params)
                .map_err(|error| Error::protocol(notification.method(), error))?,
        )
    }

    pub async fn request<R: JsonRpcRequest>(
        &self,
        request: R,
        on_response: Option<ResponseHook>,
    ) -> Result<R::Response> {
        let msg = request
            .to_untyped_message()
            .map_err(|error| Error::protocol(request.method(), error))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = RawJsonRpcMessage::request(msg.method, msg.params, id)
            .map_err(|error| Error::protocol(request.method(), error))?;
        let (waiter, response) = oneshot::channel();
        self.pending
            .lock()
            .expect("ACP pending lock poisoned")
            .insert(
                id,
                Waiter {
                    method: request.method().to_owned(),
                    response: waiter,
                    on_response,
                },
            );
        let mut guard = Pending {
            connection: self,
            id,
            sent: false,
        };
        if self.closed.is_cancelled() {
            return Err(self.closed_error());
        }
        self.send(&message)?;
        guard.sent = true;
        let result = response.await.map_err(|_| self.closed_error())??;
        R::Response::from_json(request.method(), result)
            .map_err(|error| Error::protocol(request.method(), error))
    }
}

struct Pending<'a> {
    connection: &'a Connection,
    id: i64,
    sent: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let pending = self
            .connection
            .pending
            .lock()
            .expect("ACP pending lock poisoned")
            .remove(&self.id);
        if let Some(waiter) = pending {
            if let Some(hook) = waiter.on_response {
                hook();
            }
            if self.sent {
                let _ = self
                    .connection
                    .notify(&CancelRequestNotification::new(RequestId::Number(self.id)));
            }
        }
    }
}

pub async fn read(
    stdout: ChildStdout,
    connection: Arc<Connection>,
    updates: UpdateSink,
    notifications: Option<mpsc::UnboundedSender<ClientEvent>>,
    shutdown: CancellationToken,
) {
    let mut lines = BufReader::new(stdout).lines();
    let drain = async {
        while let Some(line) = lines.next_line().await? {
            if !line.trim().is_empty() {
                dispatch(&connection, &updates, notifications.as_ref(), &line);
            }
        }
        Ok::<(), std::io::Error>(())
    };
    tokio::pin!(drain);
    let result = tokio::select! {
        result = &mut drain => Some(result),
        () = shutdown.cancelled() => tokio::time::timeout(EXIT_GRACE, &mut drain).await.ok(),
    };
    connection.close(match result {
        Some(Err(error)) => format!("reading from the harness: {error}"),
        Some(Ok(())) => "the harness closed its output".to_owned(),
        None => "the harness did not close its output".to_owned(),
    });
    connection.incoming.host.shutdown();
    shutdown.cancel();
}

fn dispatch(
    connection: &Arc<Connection>,
    updates: &UpdateSink,
    events: Option<&mpsc::UnboundedSender<ClientEvent>>,
    line: &str,
) {
    match serde_json::from_str::<RawJsonRpcMessage>(line) {
        Ok(RawJsonRpcMessage::Notification(notification)) => {
            let params = notification
                .params
                .map_or(Value::Null, RawJsonRpcParams::into_value);
            let method = &notification.method;
            if CancelRequestNotification::matches_method(method) {
                if let Ok(cancel) = CancelRequestNotification::parse_message(method, &params) {
                    connection
                        .incoming
                        .cancel_request(&RawJsonRpcId::from(cancel.request_id));
                }
            } else if AgentNotification::matches_method(method) {
                match AgentNotification::parse_message(method, &params) {
                    Ok(notification) => {
                        if let Some(events) = events {
                            let _ = events
                                .send(ClientEvent::Notification(Box::new(notification.clone())));
                        }
                        if let AgentNotification::SessionNotification(update) = notification {
                            let elapsed = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default();
                            let observed_at =
                                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
                            updates(update, observed_at);
                        }
                    }
                    Err(error) => tracing::warn!(%error, "invalid ACP notification"),
                }
            }
        }
        Ok(RawJsonRpcMessage::Request(request)) => dispatch_request(connection, request),
        Ok(RawJsonRpcMessage::Response(response)) => {
            let (id, result) = match response {
                RawJsonRpcResponse::Result { id, result } => (id, Ok(result)),
                RawJsonRpcResponse::Error { id, error } => (id, Err(error.into_acp_error())),
            };
            let RawJsonRpcId::Number(id) = id else { return };
            let waiter = connection
                .pending
                .lock()
                .expect("ACP pending lock poisoned")
                .remove(&id);
            if let Some(waiter) = waiter {
                if let Some(hook) = waiter.on_response {
                    hook();
                }
                let _ = waiter
                    .response
                    .send(result.map_err(|source| Error::protocol(&waiter.method, source)));
            }
        }
        Err(error) => tracing::warn!(%error, "invalid ACP JSON-RPC frame"),
    }
}

fn dispatch_request(connection: &Arc<Connection>, request: RawJsonRpcRequest) {
    let connection = Arc::clone(connection);
    let job = connection.incoming.prepare(request);
    tokio::spawn(async move {
        let reply = job.answer(&connection.closed).await;
        let _ = connection.send(&reply);
    });
}

pub async fn write(
    mut stdin: ChildStdin,
    mut lines: mpsc::UnboundedReceiver<String>,
    shutdown: CancellationToken,
) {
    loop {
        let line = tokio::select! {
            line = lines.recv() => line,
            () = shutdown.cancelled() => None,
        };
        let Some(line) = line else { break };
        if write_line(&mut stdin, &line).await.is_err() {
            return;
        }
    }
    lines.close();
    while let Ok(line) = lines.try_recv() {
        if write_line(&mut stdin, &line).await.is_err() {
            return;
        }
    }
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}
