//! Public ACP client handle and initialization handshake.

use std::{fmt, future::Future, path::Path, sync::Arc};

use agent_client_protocol::{JsonRpcNotification, JsonRpcRequest, schema::ProtocolVersion};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    Error, HarnessConfig,
    capabilities::Capabilities,
    connection::{self, Connection, UpdateSink},
    error::Result,
    interaction::{self, ClientEvents},
    local_io::Host,
    mcp::McpEndpoint,
    process::{self, PROCESSES},
    schema::{CancelNotification, Implementation, InitializeRequest, InitializeResponse},
};

/// Cancelled when the client drops: closes the harness's stdin.
struct Shutdown(CancellationToken);

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A running, initialized ACP harness. Dropping it shuts the process down.
pub struct Client {
    connection: Arc<Connection>,
    hello: InitializeResponse,
    capabilities: Capabilities,
    label: String,
    accepts_client_requests: bool,
    _shutdown: Shutdown,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.connection.incoming.host.shutdown();
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Launches the harness in `cwd` and negotiates ACP v1.
    ///
    /// # Errors
    /// Returns an error when the process cannot start or initialization fails.
    pub async fn start(
        config: &HarnessConfig,
        cwd: &Path,
        updates: UpdateSink,
        events: ClientEvents,
    ) -> Result<Self> {
        Self::start_with_endpoint(config, cwd, updates, events, None).await
    }

    pub(super) async fn start_with_endpoint(
        config: &HarnessConfig,
        cwd: &Path,
        updates: UpdateSink,
        events: ClientEvents,
        endpoint: Option<McpEndpoint>,
    ) -> Result<Self> {
        let label = config.command.clone();
        let host = Host::new(events.local_io.clone(), cwd, events.events.clone()).await?;
        let process::Harness {
            child,
            stdin,
            stdout,
            stderr,
        } = process::spawn(config, cwd)?;
        let (outgoing, lines) = mpsc::unbounded_channel();
        let capabilities = events.capabilities();
        let accepts_client_requests = events.requests.is_some() || endpoint.is_some();
        let notifications = events.events.clone();
        let connection = Arc::new(Connection::new(
            outgoing,
            interaction::Incoming::new(events, host, endpoint),
        ));
        let shutdown = Shutdown(CancellationToken::new());
        tokio::spawn(process::log(stderr, label.clone()));
        PROCESSES.spawn(connection::write(stdin, lines, shutdown.0.clone()));
        PROCESSES.spawn(connection::read(
            stdout,
            Arc::clone(&connection),
            updates,
            notifications,
            shutdown.0.clone(),
        ));
        PROCESSES.spawn(process::supervise(child, shutdown.0.clone(), label.clone()));

        let hello = connection
            .request(
                InitializeRequest::new(ProtocolVersion::V1)
                    .client_capabilities(capabilities)
                    .client_info(
                        Implementation::new("viz", env!("CARGO_PKG_VERSION")).title("viz"),
                    ),
                None,
            )
            .await?;
        if hello.protocol_version != ProtocolVersion::V1 {
            return Err(Error::Version(hello.protocol_version.as_u16()));
        }
        let capabilities = Capabilities::from(&hello);
        Ok(Self {
            connection,
            hello,
            capabilities,
            label,
            accepts_client_requests,
            _shutdown: shutdown,
        })
    }

    /// The negotiated capabilities and agent identity.
    #[must_use]
    pub const fn initialize_response(&self) -> &InitializeResponse {
        &self.hello
    }

    /// Capability decisions for optional methods and content.
    #[must_use]
    pub const fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Clones a handle for native steering during a bound session's active
    /// turn.
    #[must_use]
    pub fn steering(&self) -> crate::Steering {
        crate::Steering {
            connection: Arc::clone(&self.connection),
            capabilities: self.capabilities.clone(),
        }
    }

    /// The harness command, for messages.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the ACP reader has terminated.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.connection.closed.is_cancelled()
    }

    /// Resolves after stdout has been drained or the connection fails.
    pub fn closed(&self) -> impl Future<Output = String> + Send + 'static {
        let connection = Arc::clone(&self.connection);
        async move {
            connection.closed.cancelled().await;
            connection.reason()
        }
    }

    /// Sends a typed ACP request. Dropping the future sends `$/cancel_request`.
    ///
    /// # Errors
    /// Returns transport, protocol, or decoding errors.
    pub async fn request<R: JsonRpcRequest>(&self, request: R) -> Result<R::Response> {
        self.connection.request(request, None).await
    }

    pub(super) fn prompt_with_boundary(
        &self,
        request: crate::schema::PromptRequest,
        on_response: impl FnOnce() + Send + 'static,
    ) -> impl Future<Output = Result<crate::schema::PromptResponse>> + Send + 'static {
        let connection = Arc::clone(&self.connection);
        async move {
            connection
                .request(request, Some(Box::new(on_response)))
                .await
        }
    }

    pub(super) fn terminal_output(
        &self,
        session: &crate::schema::SessionId,
        terminal: &crate::schema::TerminalId,
    ) -> Option<crate::schema::TerminalOutputResponse> {
        self.connection.incoming.host.tool_output(session, terminal)
    }

    pub(super) const fn accepts_client_requests(&self) -> bool {
        self.accepts_client_requests
    }

    pub(super) fn notify(&self, notification: &impl JsonRpcNotification) -> Result<()> {
        self.connection.notify(notification)
    }

    /// Cancels the active turn and pending permission requests for that
    /// session.
    ///
    /// # Errors
    /// Returns an error when the notification cannot be sent.
    pub fn cancel(&self, session: &str) -> Result<()> {
        self.connection.incoming.cancel_session(session);
        self.connection
            .notify(&CancelNotification::new(session.to_owned()))
    }
}
