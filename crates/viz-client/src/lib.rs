//! Client for the viz backend: JSON-RPC 2.0 over a WebSocket, on the local Unix
//! socket or a remote `http://host:port` (tailnet/LAN) with a bearer token.

pub mod delta;
pub mod protocol;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::UnixStream,
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};
use viz_config::Config;

use crate::protocol::{ClientInfo, Hello, Notification, PROTOCOL_VERSION, SubmissionId, Submit};

/// Errors talking to the backend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Configuration problem (e.g. an unresolvable token).
    #[error(transparent)]
    Config(#[from] viz_config::Error),
    /// Couldn't reach the backend.
    #[error("connecting to {endpoint}: {source}")]
    Connect {
        /// The endpoint tried.
        endpoint: String,
        /// The underlying error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The backend returned a JSON-RPC error.
    #[error("{method}: backend error {code}: {message}")]
    Rpc {
        /// The method called.
        method: &'static str,
        /// JSON-RPC error code.
        code: i64,
        /// Error message.
        message: String,
    },
    /// A response didn't match the expected shape.
    #[error("{method}: unexpected response: {source}")]
    Decode {
        /// The method called.
        method: &'static str,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// The connection closed.
    #[error("connection to the backend closed")]
    Closed,
    /// The backend speaks another protocol version.
    #[error("backend speaks protocol {0}, this client speaks {PROTOCOL_VERSION}")]
    Protocol(u32),
}

/// A `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Where the backend is.
#[derive(Debug, Clone)]
pub enum Endpoint {
    /// The local Unix socket. No token: socket permissions are the auth.
    Unix(PathBuf),
    /// A remote backend's `http://host:port` and its token.
    Remote {
        /// Base URL.
        url: String,
        /// Bearer token.
        token: String,
    },
}

impl Endpoint {
    /// `[client].url` with its token if set, else the local socket.
    ///
    /// # Errors
    ///
    /// Returns an error if a remote URL is set but no token resolves.
    pub fn from_config(config: &Config) -> Result<Self> {
        let Some(url) = &config.client.url else {
            return Ok(Self::Unix(config.backend_socket()));
        };
        let token = config
            .client
            .token
            .as_ref()
            .or(config.backend.token.as_ref())
            .ok_or_else(|| {
                viz_config::Error::Secret("[client].url requires [client].token".into())
            })?
            .resolve()?;
        Ok(Self::Remote {
            url: url.trim_end_matches('/').to_owned(),
            token,
        })
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unix(path) => write!(f, "unix:{}", path.display()),
            Self::Remote { url, .. } => f.write_str(url),
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// Locks the pending-call table. Nothing panics while holding it, so poisoning
/// is ignored.
fn lock(
    pending: &Pending,
) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value>>>> {
    pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A connected client. Cheap to clone; calls may run concurrently.
#[derive(Debug, Clone)]
pub struct Client {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

/// A fresh connection: the client, its notifications, and the backend's `hello`
/// answer.
#[derive(Debug)]
pub struct Connection {
    /// Handle for calls.
    pub client: Client,
    /// Notifications pushed by the backend. Ends when the connection closes.
    pub notifications: mpsc::UnboundedReceiver<Notification>,
    /// The `hello` result.
    pub hello: Hello,
}

impl Client {
    /// Connects and says `hello`.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is unreachable, rejects the client, or
    /// speaks another protocol version.
    pub async fn connect(endpoint: &Endpoint, info: ClientInfo) -> Result<Connection> {
        let connect_err = |source: Box<dyn std::error::Error + Send + Sync>| Error::Connect {
            endpoint: endpoint.to_string(),
            source,
        };
        let (notify_tx, notifications) = mpsc::unbounded_channel();
        let pending = Pending::default();
        let outgoing = match endpoint {
            Endpoint::Unix(path) => {
                let stream = UnixStream::connect(path)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                let request = "ws://localhost/v1/rpc"
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let (ws, _) = tokio_tungstenite::client_async(request, stream)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                spawn_io(ws, pending.clone(), notify_tx)
            }
            Endpoint::Remote { url, token } => {
                let ws_url = format!("{}/v1/rpc", url.replacen("http", "ws", 1));
                let mut request = ws_url
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let auth = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| connect_err(e.into()))?;
                request.headers_mut().insert("authorization", auth);
                let (ws, _) = tokio_tungstenite::connect_async(request)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                spawn_io(ws, pending.clone(), notify_tx)
            }
        };
        let client = Self {
            outgoing,
            pending,
            next_id: Arc::default(),
        };
        let hello: Hello = client
            .call(
                "hello",
                json!({ "protocol": PROTOCOL_VERSION, "client": info }),
            )
            .await?;
        if hello.protocol != PROTOCOL_VERSION {
            return Err(Error::Protocol(hello.protocol));
        }
        Ok(Connection {
            client,
            notifications,
            hello,
        })
    }

    /// Calls `method` with `params`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] for a backend error, [`Error::Decode`] for an
    /// unexpected result, or [`Error::Closed`].
    pub async fn call<R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: impl Serialize,
    ) -> Result<R> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.outgoing
            .send(Message::text(request.to_string()))
            .map_err(|_| Error::Closed)?;
        let result = rx.await.map_err(|_| Error::Closed)?.map_err(|e| match e {
            Error::Rpc { code, message, .. } => Error::Rpc {
                method,
                code,
                message,
            },
            other => other,
        })?;
        serde_json::from_value(result).map_err(|source| Error::Decode { method, source })
    }

    /// Submits an input; see [`Submit`].
    ///
    /// # Errors
    ///
    /// See [`Client::call`].
    pub async fn submit(&self, submit: &Submit) -> Result<SubmissionId> {
        #[derive(serde::Deserialize)]
        struct Submitted {
            submission: SubmissionId,
        }
        let submitted: Submitted = self.call("conversation.submit", submit).await?;
        Ok(submitted.submission)
    }
}

fn spawn_io<S>(
    ws: WebSocketStream<S>,
    pending: Pending,
    notifications: mpsc::UnboundedSender<Notification>,
) -> mpsc::UnboundedSender<Message>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let (mut sink, mut stream) = ws.split();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(message) = rx.recv() => {
                    if sink.send(message).await.is_err() {
                        break;
                    }
                }
                incoming = stream.next() => {
                    let Some(Ok(message)) = incoming else { break };
                    let Message::Text(text) = message else { continue };
                    dispatch(&text, &pending, &notifications);
                }
                else => break,
            }
        }
        // Dropping the senders fails every pending call with `Error::Closed`.
        lock(&pending).clear();
    });
    tx
}

fn dispatch(text: &str, pending: &Pending, notifications: &mpsc::UnboundedSender<Notification>) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        tracing::warn!("backend sent invalid JSON");
        return;
    };
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let Some(tx) = lock(pending).remove(&id) else {
            return;
        };
        let result = message.get("error").map_or_else(
            || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            |error| {
                Err(Error::Rpc {
                    method: "",
                    code: error
                        .get("code")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            },
        );
        _ = tx.send(result);
    } else if message.get("method").is_some() {
        match serde_json::from_value(message) {
            Ok(notification) => _ = notifications.send(notification),
            Err(e) => tracing::debug!("ignoring unknown notification: {e}"),
        }
    }
}
