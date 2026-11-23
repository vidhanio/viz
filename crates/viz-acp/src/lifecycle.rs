//! Session setup and capability-gated client commands.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use agent_client_protocol::ErrorCode;
use viz_protocol::OptionValue;

use crate::{
    Client, Error,
    error::Result,
    schema::{
        ArchiveSessionRequest, ArchiveSessionResponse, AuthStatusRequest, AuthStatusResponse,
        CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest, DeleteSessionResponse,
        ForkSessionRequest, ForkSessionResponse, InitialSessionConfigOption, ListProvidersRequest,
        ListProvidersResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
        LoadSessionResponse, LogoutRequest, LogoutResponse, McpServer, MessageMcpNotification,
        NewSessionRequest, NewSessionResponse, ResumeSessionRequest, ResumeSessionResponse,
        SessionConfigOption, SessionConfigOptionValue, SessionId, SessionModeState,
        SessionOptionsRequest, SessionOptionsResponse, SessionStatusRequest, SessionStatusResponse,
        SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
        SetSessionModeResponse, SetSessionTitleRequest, SetSessionTitleResponse, SystemPrompt,
        SystemPromptMode, UnarchiveSessionRequest, UnarchiveSessionResponse,
    },
};

/// Inputs shared by new, load, resume, and fork.
#[derive(Clone)]
pub struct SessionSetup {
    pub cwd: PathBuf,
    pub additional_directories: Vec<PathBuf>,
    pub system_prompt: Option<SystemPrompt>,
    pub mcp_servers: Vec<McpServer>,
    pub config_options: Vec<InitialSessionConfigOption>,
}

impl SessionSetup {
    /// Starts with a workspace root and no optional session inputs.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            additional_directories: Vec::new(),
            system_prompt: None,
            mcp_servers: Vec::new(),
            config_options: Vec::new(),
        }
    }
}

impl fmt::Debug for SessionSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionSetup")
            .field("cwd", &self.cwd)
            .field("additional_directories", &self.additional_directories)
            .field("mcp_server_count", &self.mcp_servers.len())
            .finish_non_exhaustive()
    }
}

/// How to open a bound session. Replay is caller-rendered conversation context.
#[derive(Debug, Clone, Copy)]
pub enum OpenSession<'a> {
    New,
    Restore {
        id: &'a str,
        replay: Option<&'a str>,
    },
    Fork {
        id: &'a str,
        replay: Option<&'a str>,
    },
}

/// Initial state and optional handoff for the first prompt.
#[derive(Debug, Clone)]
pub struct OpenedSession {
    pub id: SessionId,
    pub modes: Option<SessionModeState>,
    pub options: Option<Vec<SessionConfigOption>>,
    pub restored: bool,
    pub handoff: Option<String>,
}

macro_rules! setup_request {
    ($request:expr, $setup:expr) => {
        $request
            .additional_directories($setup.additional_directories.clone())
            .system_prompt($setup.system_prompt.clone())
            .mcp_servers($setup.mcp_servers.clone())
            .config_options($setup.config_options.clone())
    };
}

impl Client {
    /// Opens a session using the lifecycle decision table in the crate docs.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn open(
        &self,
        setup: &SessionSetup,
        operation: OpenSession<'_>,
    ) -> Result<OpenedSession> {
        let handoff = match operation {
            OpenSession::New => None,
            OpenSession::Restore { id, replay } => {
                if self.capabilities().resume {
                    match self.resume_session(id, setup).await {
                        Ok(response) => {
                            return Ok(restored(id, response.modes, response.config_options));
                        }
                        Err(error) if method_missing(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                if self.capabilities().load {
                    match self.load_session(id, setup).await {
                        Ok(response) => {
                            return Ok(restored(id, response.modes, response.config_options));
                        }
                        Err(error) if method_missing(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                Some(replay.ok_or(Error::Unsupported("session restore without replay"))?)
            }
            OpenSession::Fork { id, replay } => {
                if self.capabilities().fork {
                    match self.fork_session(id, setup).await {
                        Ok(response) => {
                            return Ok(OpenedSession {
                                id: response.session_id,
                                modes: response.modes,
                                options: response.config_options,
                                restored: false,
                                handoff: None,
                            });
                        }
                        Err(error) if method_missing(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                Some(replay.ok_or(Error::Unsupported("session fork without replay"))?)
            }
        };
        let response = self.new_session(setup).await?;
        Ok(OpenedSession {
            id: response.session_id,
            modes: response.modes,
            options: response.config_options,
            restored: false,
            handoff: handoff.map(str::to_owned),
        })
    }

    /// Creates a session with the requested workspace and negotiated options.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn new_session(&self, setup: &SessionSetup) -> Result<NewSessionResponse> {
        self.validate_setup(setup)?;
        self.request(setup_request!(NewSessionRequest::new(&setup.cwd), setup))
            .await
    }

    /// Loads the session and streams its history through the update sink.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn load_session(
        &self,
        id: &str,
        setup: &SessionSetup,
    ) -> Result<LoadSessionResponse> {
        require(self.capabilities().load, "session/load")?;
        self.validate_setup(setup)?;
        self.request(setup_request!(
            LoadSessionRequest::new(id.to_owned(), &setup.cwd),
            setup
        ))
        .await
    }

    /// Reopens the session without replaying history.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn resume_session(
        &self,
        id: &str,
        setup: &SessionSetup,
    ) -> Result<ResumeSessionResponse> {
        require(self.capabilities().resume, "session/resume")?;
        self.validate_setup(setup)?;
        self.request(setup_request!(
            ResumeSessionRequest::new(id.to_owned(), &setup.cwd),
            setup
        ))
        .await
    }

    /// Creates an agent-native fork of the current saved session.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn fork_session(
        &self,
        id: &str,
        setup: &SessionSetup,
    ) -> Result<ForkSessionResponse> {
        require(self.capabilities().fork, "session/fork")?;
        self.validate_setup(setup)?;
        self.request(setup_request!(
            ForkSessionRequest::new(id.to_owned(), &setup.cwd),
            setup
        ))
        .await
    }

    /// Queries prompt-free session options, including legacy mode choices.
    ///
    /// # Errors
    /// Returns capability, path, or ACP request errors.
    pub async fn session_options(&self, cwd: &Path) -> Result<Option<SessionOptionsResponse>> {
        if !self.capabilities().options {
            return Ok(None);
        }
        validate_path(cwd)?;
        match self.request(SessionOptionsRequest::new(cwd)).await {
            Ok(response) => Ok(Some(response)),
            Err(error) if method_missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reads a page of sessions. Use the response cursor to fetch the next
    /// page.
    ///
    /// # Errors
    /// Returns capability or ACP request errors.
    pub async fn list_sessions(
        &self,
        request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse> {
        require(self.capabilities().list, "session/list")?;
        require(
            request.archived.is_none() || self.capabilities().archive,
            "session/list archived filter",
        )?;
        self.request(request).await
    }

    /// Changes the session's advertised mode.
    ///
    /// # Errors
    /// Returns ACP request errors.
    pub async fn set_mode(&self, session: &str, mode: &str) -> Result<SetSessionModeResponse> {
        self.request(SetSessionModeRequest::new(
            session.to_owned(),
            mode.to_owned(),
        ))
        .await
    }

    /// Sets one typed session config option.
    ///
    /// # Errors
    /// Returns ACP request errors.
    pub async fn set_option(
        &self,
        session: &str,
        id: &str,
        value: &OptionValue,
    ) -> Result<SetSessionConfigOptionResponse> {
        let value = match value {
            OptionValue::Boolean(value) => SessionConfigOptionValue::boolean(*value),
            OptionValue::Select(value) => SessionConfigOptionValue::value_id(value.clone()),
        };
        self.request(SetSessionConfigOptionRequest::new(
            session.to_owned(),
            id.to_owned(),
            value,
        ))
        .await
    }

    /// Sets a native title, or returns `None` so the caller can persist it
    /// locally.
    ///
    /// # Errors
    /// Returns ACP request errors other than method-not-found.
    pub async fn set_title(
        &self,
        session: &str,
        title: &str,
    ) -> Result<Option<SetSessionTitleResponse>> {
        if !self.capabilities().title {
            return Ok(None);
        }
        match self
            .request(SetSessionTitleRequest::new(session.to_owned(), title))
            .await
        {
            Ok(response) => Ok(Some(response)),
            Err(error) if method_missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Lists configured model providers.
    ///
    /// # Errors
    /// Returns capability or ACP request errors.
    pub async fn providers(&self) -> Result<ListProvidersResponse> {
        require(self.capabilities().providers, "providers/list")?;
        self.request(ListProvidersRequest::new()).await
    }

    /// Reads the harness's current authentication state.
    ///
    /// # Errors
    /// Returns capability or ACP request errors.
    pub async fn auth_status(&self) -> Result<AuthStatusResponse> {
        require(self.capabilities().auth_status, "auth/status")?;
        self.request(AuthStatusRequest::new()).await
    }

    /// Clears the active harness login.
    ///
    /// # Errors
    /// Returns capability or ACP request errors.
    pub async fn logout(&self) -> Result<LogoutResponse> {
        require(self.capabilities().logout, "auth/logout")?;
        self.request(LogoutRequest::new()).await
    }

    /// Emits an MCP-over-ACP notification correlated with an MCP request.
    ///
    /// # Errors
    /// Returns capability or transport errors.
    pub fn notify_mcp(&self, notification: &MessageMcpNotification) -> Result<()> {
        require(self.capabilities().mcp_acp, "MCP over ACP")?;
        self.notify(notification)
    }

    fn validate_setup(&self, setup: &SessionSetup) -> Result<()> {
        let caps = self.capabilities();
        validate_path(&setup.cwd)?;
        require(
            setup.additional_directories.is_empty() || caps.additional_directories,
            "additionalDirectories",
        )?;
        for path in &setup.additional_directories {
            validate_path(path)?;
        }
        if let Some(prompt) = &setup.system_prompt {
            require(caps.system_prompt, "systemPrompt")?;
            require(
                prompt.mode != SystemPromptMode::Override || caps.system_prompt_override,
                "systemPrompt override",
            )?;
        }
        require(
            setup.config_options.is_empty() || caps.options,
            "initial configOptions",
        )?;
        for server in &setup.mcp_servers {
            match server {
                McpServer::Http(_) => require(caps.mcp_http, "HTTP MCP")?,
                McpServer::Sse(_) => require(caps.mcp_sse, "SSE MCP")?,
                McpServer::Acp(_) => {
                    require(caps.mcp_acp, "MCP over ACP")?;
                    require(self.accepts_client_requests(), "caller MCP dispatcher")?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

macro_rules! session_command {
    ($($method:ident, $cap:ident, $name:literal, $request:ident, $response:ident);+ $(;)?) => {
        impl Client {$(
            #[doc = concat!("Sends `", $name, "` to the bound harness.")]
            ///
            /// # Errors
            /// Returns capability or ACP request errors.
            pub async fn $method(&self, id: &str) -> Result<$response> {
                require(self.capabilities().$cap, $name)?;
                self.request($request::new(id.to_owned())).await
            }
        )+}
    };
}

session_command! {
    close_session, close, "session/close", CloseSessionRequest, CloseSessionResponse;
    delete_session, delete, "session/delete", DeleteSessionRequest, DeleteSessionResponse;
    archive_session, archive, "session/archive", ArchiveSessionRequest, ArchiveSessionResponse;
    unarchive_session, archive, "session/unarchive", UnarchiveSessionRequest, UnarchiveSessionResponse;
    session_status, status, "session/status", SessionStatusRequest, SessionStatusResponse;
}

fn restored(
    id: &str,
    modes: Option<SessionModeState>,
    options: Option<Vec<SessionConfigOption>>,
) -> OpenedSession {
    OpenedSession {
        id: SessionId::new(id.to_owned()),
        modes,
        options,
        restored: true,
        handoff: None,
    }
}

pub const fn require(supported: bool, capability: &'static str) -> Result<()> {
    if supported {
        Ok(())
    } else {
        Err(Error::Unsupported(capability))
    }
}

pub fn method_missing(error: &Error) -> bool {
    matches!(error, Error::Protocol { source, .. } if source.code == ErrorCode::METHOD_NOT_FOUND)
}

fn validate_path(path: &Path) -> Result<()> {
    if path.is_absolute() && path.to_str().is_some() {
        Ok(())
    } else {
        Err(Error::Path(path.to_owned()))
    }
}
