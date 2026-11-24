//! The connection: JSON-RPC requests out, responses and notifications in, over
//! one WebSocket driven by a background task.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message, Utf8Bytes, client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig,
    },
};

use crate::{
    Endpoint, Error, Result,
    protocol::{
        ClientInfo, ConfigureThread, CreateProject, DeleteThread, EntryPage, Harness,
        HarnessOptions, HarnessOptionsResult, Hello, HelloParams, LaunchThread, ListEntries,
        ListHarnesses, ListProjects, ListThreads, Method, Notification, PROTOCOL_VERSION, Project,
        RemoveProject, RemoveQueuedInput, RunId, SearchFiles, SearchHit, SearchThreads, SendInput,
        StopThread, ThreadId, ThreadSummary, Unwatch, UpdateProject, UpdateThread, Watch, limits,
    },
};

/// The WebSocket path the server serves the protocol on.
const RPC_PATH: &str = "/v2/rpc";
/// Bounded outgoing WebSocket request queue capacity.
const OUTGOING_CAPACITY: usize = 256;
/// Bounded incoming notification queue capacity before triggering a reconnect
/// resync.
const NOTIFICATION_CAPACITY: usize = 1024;

/// A connected client. Cheap to clone; calls may run concurrently.
#[derive(Debug, Clone)]
pub struct Client {
    outgoing: mpsc::Sender<Message>,
    pending: Arc<PendingCalls>,
    next_id: Arc<AtomicU64>,
}

/// A fresh connection: the client, its notifications, and the server's `hello`
/// answer.
#[derive(Debug)]
pub struct Connection {
    /// Handle for calls.
    pub client: Client,
    /// Notifications pushed by the server. Ends when the connection closes.
    pub notifications: mpsc::Receiver<Notification>,
    /// The `hello` result.
    pub hello: Hello,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client>();
    assert_send_sync::<Connection>();
};

impl Client {
    /// Connects and says `hello`. The connection's I/O runs on a task spawned
    /// onto the current Tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is unreachable, rejects the client, or
    /// speaks another protocol version.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub async fn connect(
        endpoint: &Endpoint,
        info: ClientInfo,
        environment: Option<String>,
    ) -> Result<Connection> {
        let connect_err = |source: Box<dyn std::error::Error + Send + Sync>| Error::Connect {
            endpoint: endpoint.to_string(),
            source,
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (notify_tx, notifications) = mpsc::channel(NOTIFICATION_CAPACITY);
        let pending = Arc::<PendingCalls>::default();
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(limits::MAX_WS_FRAME))
            .max_frame_size(Some(limits::MAX_WS_FRAME));
        let outgoing = match endpoint {
            Endpoint::Unix { path } => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                let request = format!("ws://localhost{RPC_PATH}")
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let (ws, _) =
                    tokio_tungstenite::client_async_with_config(request, stream, Some(ws_config))
                        .await
                        .map_err(|e| connect_err(e.into()))?;
                spawn_io(ws, Arc::clone(&pending), notify_tx)
            }
            Endpoint::Http { url, token } => {
                let ws_url = format!("ws{}{RPC_PATH}", &url[4..]);
                let mut request = ws_url
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let auth = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| connect_err(e.into()))?;
                request.headers_mut().insert("authorization", auth);
                let (ws, _) =
                    tokio_tungstenite::connect_async_with_config(request, Some(ws_config), false)
                        .await
                        .map_err(|e| connect_err(e.into()))?;
                spawn_io(ws, Arc::clone(&pending), notify_tx)
            }
        };
        let client = Self {
            outgoing,
            pending,
            next_id: Arc::default(),
        };
        let hello = client
            .request(&HelloParams {
                protocol: PROTOCOL_VERSION,
                client: info,
                environment,
            })
            .await?;
        if hello.protocol != PROTOCOL_VERSION {
            return Err(Error::Protocol {
                server: hello.protocol,
            });
        }
        tracing::debug!(%endpoint, environment = %hello.environment, "connected to viz server");
        Ok(Connection {
            client,
            notifications,
            hello,
        })
    }

    /// Closes this WebSocket and fails calls that are still pending.
    pub fn close(&self) {
        if self.outgoing.try_send(Message::Close(None)).is_err() {
            let outgoing = self.outgoing.clone();
            tokio::spawn(async move {
                let _ = outgoing.send(Message::Close(None)).await;
            });
        }
    }

    /// Lists every thread, titled by the server.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn list_threads(&self) -> Result<Vec<ThreadSummary>> {
        Ok(self.request(&ListThreads::default()).await?.threads)
    }

    /// Launches a thread.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn launch_thread(&self, params: &LaunchThread) -> Result<ThreadId> {
        Ok(self.request(params).await?.thread)
    }

    /// Changes a thread's title or project.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn update_thread(&self, params: &UpdateThread) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Sets a thread's configuration.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn configure_thread(&self, params: &ConfigureThread) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Stops a thread's active turn.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn stop_thread(&self, thread: ThreadId) -> Result<()> {
        self.request(&StopThread { thread, run: None })
            .await
            .map(drop)
    }

    /// Deletes a thread without rewriting or deleting other threads.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn delete_thread(&self, params: &DeleteThread) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Lists available project directories.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        Ok(self.request(&ListProjects {}).await?.projects)
    }

    /// Adds a project directory.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn create_project(&self, params: &CreateProject) -> Result<Project> {
        Ok(self.request(params).await?.project)
    }

    /// Browse the server’s directories and complete a typed path.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn browse_folders(
        &self,
        args: &viz_protocol::BrowseFolders,
    ) -> Result<viz_protocol::FolderEntries> {
        self.request(args).await
    }

    /// Create a directory on the server.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn create_folder(
        &self,
        args: &viz_protocol::CreateFolder,
    ) -> Result<viz_protocol::FolderCreated> {
        self.request(args).await
    }

    /// Initialize Git in a server directory.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn init_repository(
        &self,
        args: &viz_protocol::InitRepository,
    ) -> Result<viz_protocol::FolderCreated> {
        self.request(args).await
    }

    /// Start cloning and return a progress handle.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn start_clone(
        &self,
        args: &viz_protocol::StartClone,
    ) -> Result<viz_protocol::CloneStatus> {
        self.request(args).await
    }

    /// Read clone progress or its terminal result.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn clone_status(
        &self,
        args: &viz_protocol::ReadClone,
    ) -> Result<viz_protocol::CloneStatus> {
        self.request(args).await
    }

    /// Request cancellation of an in-progress clone.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn cancel_clone(
        &self,
        args: &viz_protocol::CancelClone,
    ) -> Result<viz_protocol::CloneStatus> {
        self.request(args).await
    }

    /// List or search repositories the authenticated GitHub viewer owns.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn github_repositories(
        &self,
        args: &viz_protocol::GitHubRepositories,
    ) -> Result<viz_protocol::GitHubRepositoryList> {
        self.request(args).await
    }

    /// File a projectless thread and its scratch directory under a new project.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn promote_thread(
        &self,
        args: &viz_protocol::PromoteThread,
    ) -> Result<viz_protocol::PromotedThread> {
        self.request(args).await
    }

    /// Get a project's image icon, or confirm that the caller’s `ETag` still
    /// matches.
    ///
    /// # Errors
    /// Returns an error when the server cannot resolve the icon.
    pub async fn project_icon(
        &self,
        params: &viz_protocol::ReadProjectIcon,
    ) -> Result<viz_protocol::ProjectIconResult> {
        self.request(params).await
    }

    /// Updates a project's name and preferences.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn update_project(&self, params: &UpdateProject) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Removes a project.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn remove_project(&self, params: &RemoveProject) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Lists configured ACP runners.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn list_harnesses(&self) -> Result<Vec<Harness>> {
        Ok(self.request(&ListHarnesses {}).await?.harnesses)
    }

    /// Fetches the available settings for a model without creating a
    /// thread.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn harness_options(&self, params: &HarnessOptions) -> Result<HarnessOptionsResult> {
        self.request(params).await
    }

    /// Finds files in the thread's working directory.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn search_files(
        &self,
        params: &SearchFiles,
    ) -> Result<Vec<viz_protocol::FileSearchMatch>> {
        Ok(self.request(params).await?.files)
    }

    /// Searches transcript text across threads.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn search_threads(&self, params: &SearchThreads) -> Result<Vec<SearchHit>> {
        Ok(self.request(params).await?.hits)
    }

    /// Reads a page of earlier transcript entries.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn list_entries(&self, params: &ListEntries) -> Result<EntryPage> {
        self.request(params).await
    }

    /// Removes an input that has not yet been dispatched.
    ///
    /// # Errors
    /// See [`Client::send`].
    pub async fn remove_queued_input(&self, params: &RemoveQueuedInput) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Sends an input and returns its run ID.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] for a server error, [`Error::Encode`] or
    /// [`Error::Decode`] for a request or response that isn't valid protocol,
    /// or [`Error::Closed`].
    pub async fn send(&self, send: &SendInput) -> Result<RunId> {
        Ok(self.request(send).await?.run)
    }

    /// Starts receiving [`Notification::ThreadView`] for
    /// `thread`, beginning with its full view.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn watch(&self, thread: ThreadId) -> Result<()> {
        self.request(&Watch { thread }).await.map(drop)
    }

    /// Stops watching `thread`.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn unwatch(&self, thread: ThreadId) -> Result<()> {
        self.request(&Unwatch { thread }).await.map(drop)
    }

    /// Calls the method `params` belong to and decodes its result.
    ///
    /// # Errors
    ///
    /// See [`Client::send`].
    pub async fn request<M: Method>(&self, params: &M) -> Result<M::Response> {
        let method = M::NAME;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = serde_json::to_string(&Request {
            jsonrpc: "2.0",
            id,
            method,
            params,
        })
        .map_err(|source| Error::Encode { method, source })?;
        let (tx, rx) = oneshot::channel();
        let _guard = PendingGuard::insert(&self.pending, id, tx);
        if self.outgoing.send(Message::text(request)).await.is_err() {
            return Err(Error::Closed);
        }
        let raw = rx.await.map_err(|_| Error::Closed)?;
        let response: Response<M::Response> = serde_json::from_str(raw.as_str())
            .map_err(|source| Error::Decode { method, source })?;
        if let Some(ErrorObject {
            code,
            message,
            data,
        }) = response.error
        {
            return Err(Error::Rpc {
                method,
                code,
                message,
                git_failure: data.and_then(|data| serde_json::from_value(data).ok()),
            });
        }
        response.result.ok_or_else(|| Error::Decode {
            method,
            source: serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response missing result or error",
            )),
        })
    }
}

/// A JSON-RPC request.
#[derive(Serialize)]
struct Request<'a, P> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: &'a P,
}

/// A JSON-RPC response, less its `id`.
#[derive(Deserialize)]
struct Response<R> {
    result: Option<R>,
    error: Option<ErrorObject>,
}

/// A JSON-RPC error.
#[derive(Deserialize)]
struct ErrorObject {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<serde_json::Value>,
}

/// Where a response goes: its call, or the decoding error in its place.
type Responder = oneshot::Sender<Utf8Bytes>;

/// Calls awaiting their responses, by request id.
#[derive(Default)]
struct PendingCalls(Mutex<HashMap<u64, Responder>>);

impl std::fmt::Debug for PendingCalls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCalls")
            .field("len", &self.lock().len())
            .finish()
    }
}

impl PendingCalls {
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Responder>> {
        self.0.lock().expect("pending-call table lock poisoned")
    }

    fn insert(&self, id: u64, responder: Responder) {
        self.lock().insert(id, responder);
    }

    fn take(&self, id: u64) -> Option<Responder> {
        self.lock().remove(&id)
    }

    /// Fails every pending call with [`Error::Closed`] by dropping its
    /// responder.
    fn close(&self) {
        self.lock().clear();
    }
}

/// Removes a pending RPC entry when its caller future drops before a reply
/// arrives.
struct PendingGuard<'a> {
    pending: &'a PendingCalls,
    id: u64,
}

impl<'a> PendingGuard<'a> {
    fn insert(pending: &'a PendingCalls, id: u64, responder: Responder) -> Self {
        pending.insert(id, responder);
        Self { pending, id }
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        let _ = self.pending.take(self.id);
    }
}

/// Spawns the task that drives `ws`; returns the bounded queue its writes go
/// to.
fn spawn_io<S>(
    ws: WebSocketStream<S>,
    pending: Arc<PendingCalls>,
    notifications: mpsc::Sender<Notification>,
) -> mpsc::Sender<Message>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Message>(OUTGOING_CAPACITY);
    let (mut sink, mut stream) = ws.split();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    let Some(message) = outgoing else { break };
                    let closing = matches!(message, Message::Close(_));
                    if sink.send(message).await.is_err() || closing {
                        break;
                    }
                }
                incoming = stream.next() => {
                    let Some(Ok(message)) = incoming else { break };
                    let Message::Text(text) = message else { continue };
                    if !dispatch(text, &pending, &notifications) {
                        break;
                    }
                }
            }
        }
        drop(rx);
        pending.close();
    });
    tx
}

#[derive(Deserialize)]
struct Route {
    id: Option<u64>,
    method: Option<IgnoredAny>,
}

/// Routes one incoming message: a response to its call, a notification to the
/// bounded notification queue. Returns `false` when notifications lagged full
/// so the caller reconnects and resyncs.
fn dispatch(
    text: Utf8Bytes,
    pending: &PendingCalls,
    notifications: &mpsc::Sender<Notification>,
) -> bool {
    let route: Route = match serde_json::from_str(text.as_str()) {
        Ok(message) => message,
        Err(e) => {
            tracing::warn!("server sent invalid JSON: {e}");
            return true;
        }
    };
    if let Some(id) = route.id {
        if let Some(responder) = pending.take(id) {
            _ = responder.send(text);
        }
    } else if route.method.is_some() {
        match serde_json::from_str(text.as_str()) {
            Ok(notification) => match notifications.try_send(notification) {
                Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!("client notification queue overflowed; closing to resync");
                    return false;
                }
            },
            Err(e) => tracing::debug!("ignoring unknown notification: {e}"),
        }
    }
    true
}
