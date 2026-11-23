//! Binds a process to one ACP session and opens its update stream.

mod configure;
mod drive;
mod state;

use std::{
    fmt,
    sync::{Arc, Mutex},
};

pub use drive::{Outcome, Prompt, stop_reason_name};
pub use state::SessionSnapshot;
use state::{State, lock, update_sink};
use tokio::sync::{mpsc, watch};

use crate::{
    Client, ClientEvents, Error, HarnessConfig, ObservedUpdate, OpenSession, SessionSetup, bridge,
    error::Result,
    mcp::McpEndpoint,
    schema::{EnvVariable, HttpHeader, McpServer, McpServerHttp, McpServerStdio},
};

/// A thread's open ACP session. Dropping it stops the harness.
pub struct Session {
    agent: Client,
    state: Arc<Mutex<State>>,
    changes: Arc<watch::Sender<()>>,
    idle: mpsc::UnboundedReceiver<ObservedUpdate>,
    id: String,
    config: HarnessConfig,
    restored: bool,
    /// A prompt was sent, so a new session is one the harness has saved.
    prompted: bool,
    mcp: Option<McpEndpoint>,
    /// A failed or abandoned turn left the session in an unknown state.
    broken: bool,
    handoff: Option<String>,
}

impl Session {
    /// Starts the harness in `cwd` and opens the thread's session:
    /// `session/resume` or `session/load` when `saved` is given, otherwise
    /// `session/new`. `mcp` gives the agent viz's tools.
    ///
    /// Dropping the future stops the harness.
    ///
    /// # Errors
    /// Returns handshake or session request errors.
    pub async fn open(
        config: &HarnessConfig,
        setup: SessionSetup,
        saved: Option<&str>,
        mcp: Option<&McpEndpoint>,
    ) -> Result<Self> {
        let operation = saved.map_or(OpenSession::New, |id| OpenSession::Restore {
            id,
            replay: None,
        });
        Self::open_with(config, setup, operation, mcp, ClientEvents::default()).await
    }

    /// Starts a process and opens, restores, or forks a session.
    ///
    /// # Errors
    /// Returns handshake, setup, or session request errors.
    pub async fn open_with(
        config: &HarnessConfig,
        mut setup: SessionSetup,
        operation: OpenSession<'_>,
        mcp: Option<&McpEndpoint>,
        events: ClientEvents,
    ) -> Result<Self> {
        let state = Arc::new(Mutex::new(State::default()));
        let changes = Arc::new(watch::channel(()).0);
        let agent = Client::start_with_endpoint(
            config,
            &setup.cwd,
            update_sink(&state, &changes),
            events,
            mcp.cloned(),
        )
        .await?;
        if let Some(endpoint) = mcp
            && !setup.mcp_servers.contains(&endpoint.acp_server())
        {
            setup.mcp_servers.push(declaration(&agent, endpoint)?);
        }
        if !agent.capabilities().additional_directories {
            setup.additional_directories.clear();
        }
        let opened = agent.open(&setup, operation).await?;
        let session_id = opened.id.0.to_string();
        let idle = lock(&state).bind(&session_id, opened.modes, opened.options);
        Ok(Self {
            agent,
            state,
            changes,
            idle,
            id: session_id,
            config: config.clone(),
            restored: opened.restored,
            prompted: false,
            mcp: mcp.cloned(),
            broken: false,
            handoff: opened.handoff,
        })
    }

    /// The low-level client for operations outside the turn driver.
    #[must_use]
    pub const fn client(&self) -> &Client {
        &self.agent
    }

    /// Whether stopping the harness now loses nothing: the harness can
    /// reopen the session, and has saved it.
    #[must_use]
    pub const fn is_restorable(&self) -> bool {
        self.agent.capabilities().can_restore() && (self.restored || self.prompted)
    }

    /// The runner's ACP session ID, to persist so the thread reopens the same
    /// session.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Whether the session can take another turn.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.broken && !self.agent.is_closed()
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}

/// Declares the scoped endpoint using HTTP, ACP, or the executable's stdio
/// bridge.
fn declaration(agent: &Client, endpoint: &McpEndpoint) -> Result<McpServer> {
    if agent.capabilities().mcp_http {
        return Ok(McpServer::Http(
            McpServerHttp::new("viz", endpoint.url()).headers(vec![HttpHeader::new(
                "Authorization",
                format!("Bearer {}", endpoint.token()),
            )]),
        ));
    }
    if agent.capabilities().mcp_acp {
        return Ok(endpoint.acp_server());
    }
    let executable = std::env::current_exe().map_err(|source| Error::Io {
        operation: "locating the MCP bridge executable",
        source,
    })?;
    Ok(McpServer::Stdio(
        McpServerStdio::new("viz", executable)
            .args(vec!["mcp-bridge".to_owned()])
            .env(vec![
                EnvVariable::new(bridge::URL_ENV, endpoint.url()),
                EnvVariable::new(bridge::TOKEN_ENV, endpoint.token()),
            ]),
    ))
}
