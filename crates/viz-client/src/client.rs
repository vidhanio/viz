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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::UnixStream,
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};

use crate::{
    Endpoint, Error, Result,
    protocol::{
        AbortConversation, Branches, ClientInfo, CommitResult, ConfigureConversation,
        ConversationId, ConversationSummary, CreateConversation, CreateProject, DeleteConversation,
        DiffFile, EntryPage, GitBranches, GitCheckout, GitCommit, GitDiff, GitPr, GitPush,
        GitRevert, Harness, HarnessOptions, HarnessOptionsResult, Hello, HelloParams,
        ListConversations, ListEntries, ListHarnesses, ListProjects, ListWorktrees,
        ManagedWorktree, Method, Notification, PROTOCOL_VERSION, PrResult, Project, RemoveProject,
        RemoveQueuedInput, SearchConversations, SearchFiles, SearchHit, SubmissionId, Submit,
        Unwatch, UpdateConversation, UpdateProject, Watch,
    },
};

/// The WebSocket path the backend serves the protocol on.
const RPC_PATH: &str = "/v1/rpc";

/// A connected client. Cheap to clone; calls may run concurrently.
#[derive(Debug, Clone)]
pub struct Client {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Arc<PendingCalls>,
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
    /// Returns an error if the backend is unreachable, rejects the client, or
    /// speaks another protocol version.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub async fn connect(endpoint: &Endpoint, info: ClientInfo) -> Result<Connection> {
        let connect_err = |source: Box<dyn std::error::Error + Send + Sync>| Error::Connect {
            endpoint: endpoint.to_string(),
            source,
        };
        let (notify_tx, notifications) = mpsc::unbounded_channel();
        let pending = Arc::<PendingCalls>::default();
        let outgoing = match endpoint {
            Endpoint::Unix(path) => {
                let stream = UnixStream::connect(path)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                let request = format!("ws://localhost{RPC_PATH}")
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let (ws, _) = tokio_tungstenite::client_async(request, stream)
                    .await
                    .map_err(|e| connect_err(e.into()))?;
                spawn_io(ws, Arc::clone(&pending), notify_tx)
            }
            Endpoint::Remote { url, token } => {
                let ws_url = format!("{}{RPC_PATH}", url.replacen("http", "ws", 1));
                let mut request = ws_url
                    .into_client_request()
                    .map_err(|e| connect_err(e.into()))?;
                let auth = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| connect_err(e.into()))?;
                request.headers_mut().insert("authorization", auth);
                let (ws, _) = tokio_tungstenite::connect_async(request)
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
            })
            .await?;
        if hello.protocol != PROTOCOL_VERSION {
            return Err(Error::Protocol {
                backend: hello.protocol,
            });
        }
        Ok(Connection {
            client,
            notifications,
            hello,
        })
    }

    /// Lists every conversation, titled by the backend.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        Ok(self.request(&ListConversations {}).await?.conversations)
    }

    /// Creates a chat.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn create_conversation(&self, params: &CreateConversation) -> Result<ConversationId> {
        Ok(self.request(params).await?.conversation)
    }

    /// Changes a conversation's title, archive status, or project.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn update_conversation(&self, params: &UpdateConversation<'_>) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Sets a conversation's configuration.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn configure_conversation(&self, params: &ConfigureConversation<'_>) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Stops a conversation's active turn.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn abort_conversation(&self, conversation: ConversationId) -> Result<()> {
        self.request(&AbortConversation { conversation })
            .await
            .map(drop)
    }

    /// Deletes a conversation and its subagents, returning the worktree
    /// paths left on disk because they have uncommitted changes.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn delete_conversation(&self, conversation: ConversationId) -> Result<Vec<String>> {
        Ok(self
            .request(&DeleteConversation { conversation })
            .await?
            .kept)
    }

    /// Lists available project directories.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        Ok(self.request(&ListProjects {}).await?.projects)
    }

    /// Adds a project directory.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn create_project(&self, params: &CreateProject<'_>) -> Result<Project> {
        Ok(self.request(params).await?.project)
    }

    /// Updates a project's name or default workspace.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn update_project(&self, params: &UpdateProject<'_>) -> Result<Project> {
        Ok(self.request(params).await?.project)
    }

    /// Removes a project.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn remove_project(&self, params: &RemoveProject<'_>) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Lists managed worktrees available to reuse in a project.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn list_worktrees(&self, project: &str) -> Result<Vec<ManagedWorktree>> {
        Ok(self.request(&ListWorktrees { project }).await?.worktrees)
    }

    /// Lists configured ACP runners.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn list_harnesses(&self) -> Result<Vec<Harness>> {
        Ok(self.request(&ListHarnesses {}).await?.harnesses)
    }

    /// Fetches the available settings for a model without creating a
    /// conversation.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn harness_options(&self, params: &HarnessOptions) -> Result<HarnessOptionsResult> {
        self.request(params).await
    }

    /// Finds files in the conversation's working directory.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn search_files(&self, params: &SearchFiles<'_>) -> Result<Vec<String>> {
        Ok(self.request(params).await?.files)
    }

    /// Searches transcript text across conversations.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn search_conversations(
        &self,
        params: &SearchConversations<'_>,
    ) -> Result<Vec<SearchHit>> {
        Ok(self.request(params).await?.hits)
    }

    /// Reads a page of earlier transcript entries.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn list_entries(&self, params: &ListEntries) -> Result<EntryPage> {
        self.request(params).await
    }

    /// Removes an input that has not yet been dispatched.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn remove_queued_input(&self, params: &RemoveQueuedInput) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Gets unified patches and their file metadata.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_diff(&self, params: &GitDiff<'_>) -> Result<Vec<DiffFile>> {
        Ok(self.request(params).await?.files)
    }

    /// Lists branches for the conversation's repository.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_branches(&self, target: GitBranches<'_>) -> Result<Branches> {
        self.request(&target).await
    }

    /// Switches branches or creates and checks out a new branch.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_checkout(&self, params: &GitCheckout<'_>) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Commits changes in the conversation's repository.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_commit(&self, params: &GitCommit<'_>) -> Result<CommitResult> {
        self.request(params).await
    }

    /// Pushes the active branch.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_push(&self, conversation: ConversationId) -> Result<()> {
        self.request(&GitPush { conversation }).await.map(drop)
    }

    /// Creates a pull request for the active branch.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_pr(&self, params: &GitPr<'_>) -> Result<PrResult> {
        self.request(params).await
    }

    /// Restores the working tree as it was at the start of a turn.
    ///
    /// # Errors
    /// See [`Client::submit`].
    pub async fn git_revert(&self, params: &GitRevert) -> Result<()> {
        self.request(params).await.map(drop)
    }

    /// Submits an input; see [`Submit`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rpc`] for a backend error, [`Error::Encode`] or
    /// [`Error::Decode`] for a request or response that isn't valid protocol,
    /// or [`Error::Closed`].
    pub async fn submit(&self, submit: &Submit) -> Result<SubmissionId> {
        Ok(self.request(submit).await?.submission)
    }

    /// Starts receiving [`Notification::ConversationView`] for
    /// `conversation`, beginning with its full view.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn watch(&self, conversation: ConversationId) -> Result<()> {
        self.request(&Watch { conversation }).await.map(drop)
    }

    /// Stops watching `conversation`.
    ///
    /// # Errors
    ///
    /// See [`Client::submit`].
    pub async fn unwatch(&self, conversation: ConversationId) -> Result<()> {
        self.request(&Unwatch { conversation }).await.map(drop)
    }

    /// Calls the method `params` belong to and decodes its result.
    async fn request<M: Method>(&self, params: &M) -> Result<M::Response> {
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
        self.pending.insert(id, tx);
        if self.outgoing.send(Message::text(request)).is_err() {
            self.pending.take(id);
            return Err(Error::Closed);
        }
        let response = rx
            .await
            .map_err(|_| Error::Closed)?
            .map_err(|source| Error::Decode { method, source })?;
        if let Some(ErrorObject { code, message }) = response.error {
            return Err(Error::Rpc {
                method,
                code,
                message,
            });
        }
        serde_json::from_value(response.result).map_err(|source| Error::Decode { method, source })
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
struct Response {
    #[serde(default)]
    result: Value,
    error: Option<ErrorObject>,
}

/// A JSON-RPC error.
#[derive(Deserialize)]
struct ErrorObject {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

/// Where a response goes: its call, or the decoding error in its place.
type Responder = oneshot::Sender<serde_json::Result<Response>>;

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

/// Spawns the task that drives `ws`; returns the queue its writes go to.
fn spawn_io<S>(
    ws: WebSocketStream<S>,
    pending: Arc<PendingCalls>,
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
        // Close the queue first: a call made from here on fails to send instead
        // of waiting on a responder nobody will answer.
        drop(rx);
        pending.close();
    });
    tx
}

/// Routes one incoming message: a response to its call, a notification to the
/// notification queue.
fn dispatch(
    text: &str,
    pending: &PendingCalls,
    notifications: &mpsc::UnboundedSender<Notification>,
) {
    let message: Value = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(e) => {
            tracing::warn!("backend sent invalid JSON: {e}");
            return;
        }
    };
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        if let Some(responder) = pending.take(id) {
            _ = responder.send(serde_json::from_value(message));
        }
    } else if message.get("method").is_some() {
        match serde_json::from_value(message) {
            Ok(notification) => _ = notifications.send(notification),
            Err(e) => tracing::debug!("ignoring unknown notification: {e}"),
        }
    }
}
