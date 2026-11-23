//! The loopback MCP server that dispatches caller-supplied tools.
//!
//! rmcp serves Streamable HTTP statelessly, for both the 2026-07-28 protocol
//! and 2025-era clients, on a loopback port of its own. Each thread's session
//! gets its own bearer token, which is its identity: a tool call acts as the
//! thread that holds the token, never as an id the caller claims. The token is
//! checked before rmcp sees the request. Agents without HTTP MCP reach this
//! endpoint through the stdio bridge (`super::bridge`).

use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse as _, Response},
};
use futures_util::future::BoxFuture;
use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
        EmptyObject, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerConfig, Tool,
    },
    service::{RequestContext, RoleServer},
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use serde::Serialize;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use viz_protocol::{RunId, ThreadId};

use crate::{
    Error,
    error::Result,
    receipts::Receipts,
    schema::{
        McpError, McpServer, McpServerAcp, McpServerAcpId, MessageMcpRequest, MessageMcpResponse,
    },
};
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolScope {
    pub thread: ThreadId,
    pub run: RunId,
}

#[derive(Debug)]
pub struct ToolResult {
    pub text: String,
    pub child: Option<ThreadId>,
    pub child_environment: Option<String>,
}

const PATH: &str = "/mcp";
const INSTRUCTIONS: &str = "viz runs you in a thread. Delegate work to child threads with delegate_task, and read or drive other threads with the thread tools.";

/// Runs viz's tools for an authenticated MCP call.
pub trait Dispatch: Send + Sync + 'static {
    fn call(
        &self,
        scope: ToolScope,
        name: String,
        args: Value,
    ) -> BoxFuture<'static, std::result::Result<ToolResult, String>>;
}

/// The MCP server. Cheap to clone; it stops on [`Mcp::stop`] or when the last
/// clone and endpoint drop.
#[derive(Clone)]
pub struct Mcp {
    inner: Arc<Inner>,
}

struct Inner {
    url: String,
    tools: Arc<[Tool]>,
    grants: Mutex<HashMap<String, Arc<Grant>>>,
    /// Held weakly: the dispatcher owns the workspace, which owns this server.
    dispatch: OnceLock<Weak<dyn Dispatch>>,
    shutdown: CancellationToken,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// What a token may do: act as its thread, during that thread's running turn.
struct Grant {
    thread: ThreadId,
    active: Mutex<Option<Active>>,
}

struct Active {
    run: RunId,
    cancel: CancellationToken,
    receipts: Arc<Receipts>,
}

impl Mcp {
    /// Listens on a free loopback port. Tool calls fail until
    /// [`Mcp::set_dispatch`].
    ///
    /// # Errors
    /// Returns an error for an invalid tool schema or an unavailable listener.
    pub async fn start(definitions: Vec<ToolDefinition>) -> Result<Self> {
        let tools: Arc<[Tool]> = definitions
            .into_iter()
            .map(|definition| {
                let Value::Object(schema) = definition.input_schema else {
                    return Err(Error::ToolSchema(definition.name));
                };
                Ok(Tool::new(
                    definition.name,
                    definition.description,
                    Arc::new(schema),
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|source| Error::Io {
                operation: "binding the MCP server's loopback port",
                source,
            })?;
        let address = listener.local_addr().map_err(|source| Error::Io {
            operation: "reading the MCP server's address",
            source,
        })?;
        let shutdown = CancellationToken::new();
        let inner = Arc::new(Inner {
            url: format!("http://{address}{PATH}"),
            tools: Arc::clone(&tools),
            grants: Mutex::default(),
            dispatch: OnceLock::new(),
            shutdown: shutdown.clone(),
        });
        let handler = Handler {
            tools,
            inner: Arc::downgrade(&inner),
        };
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .enforce_origin_validation()
            .with_cancellation_token(shutdown.child_token());
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(NeverSessionManager::default()),
            config,
        );
        let router =
            Router::new()
                .nest_service(PATH, service)
                .layer(middleware::from_fn_with_state(
                    Arc::downgrade(&inner),
                    authenticate,
                ));
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
            {
                tracing::error!("MCP server: {error}");
            }
        });
        Ok(Self { inner })
    }

    /// Routes tool calls to `dispatch`, held weakly; the caller keeps it alive.
    pub fn set_dispatch(&self, dispatch: &Arc<dyn Dispatch>) {
        if self.inner.dispatch.set(Arc::downgrade(dispatch)).is_err() {
            tracing::warn!("MCP dispatch was already set");
        }
    }

    /// A new token acting as `thread`, revoked when the last clone of the
    /// endpoint drops.
    ///
    /// # Panics
    /// Panics if the MCP grants lock is poisoned.
    #[must_use]
    pub fn endpoint(&self, thread: ThreadId) -> McpEndpoint {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let grant = Arc::new(Grant {
            thread,
            active: Mutex::new(None),
        });
        self.inner
            .grants
            .lock()
            .expect("MCP grants lock poisoned")
            .insert(token.clone(), Arc::clone(&grant));
        McpEndpoint {
            inner: Arc::new(Endpoint {
                mcp: Arc::clone(&self.inner),
                token,
                grant,
                acp_id: McpServerAcpId::new(Uuid::new_v4().simple().to_string()),
            }),
        }
    }

    /// Stops serving; open endpoints stop working.
    pub fn stop(&self) {
        self.inner.shutdown.cancel();
    }
}

/// One thread's MCP identity: the server URL and its bearer token.
#[derive(Clone)]
pub struct McpEndpoint {
    inner: Arc<Endpoint>,
}

struct Endpoint {
    mcp: Arc<Inner>,
    token: String,
    grant: Arc<Grant>,
    acp_id: McpServerAcpId,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.mcp
            .grants
            .lock()
            .expect("MCP grants lock poisoned")
            .remove(&self.token);
    }
}

impl McpEndpoint {
    #[must_use]
    pub fn url(&self) -> &str {
        &self.inner.mcp.url
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.inner.token
    }

    /// Declares this endpoint using the ACP MCP transport.
    #[must_use]
    pub fn acp_server(&self) -> McpServer {
        McpServer::Acp(McpServerAcp::new("viz", self.inner.acp_id.clone()))
    }

    pub(super) fn owns_acp_request(&self, request: &MessageMcpRequest) -> bool {
        request.server_id == self.inner.acp_id
    }

    /// Dispatches one ACP MCP request with the same scope as HTTP MCP.
    ///
    /// # Errors
    /// Returns an ACP error for an unknown binding, stopped server, or encoding
    /// failure. MCP method and tool errors are carried in the response.
    pub async fn call_acp(
        &self,
        request: &MessageMcpRequest,
    ) -> std::result::Result<MessageMcpResponse, agent_client_protocol::Error> {
        use agent_client_protocol::{Error, ErrorCode};

        if !self.owns_acp_request(request) {
            return Err(Error::invalid_params().data("unknown MCP server binding"));
        }
        let inner = &self.inner.mcp;
        if inner.shutdown.is_cancelled() {
            return Err(Error::new(
                ErrorCode::INTERNAL_ERROR,
                "the MCP endpoint is stopped",
            ));
        }
        let result = match request.method.as_str() {
            "server/discover" => serde_json::to_value(DiscoverResult::from_server_info(
                vec![ProtocolVersion::V_2026_07_28],
                server_info(),
            )),
            "tools/list" => {
                serde_json::to_value(ListToolsResult::with_all_items(inner.tools.to_vec()))
            }
            "tools/call" => {
                let request = Value::Object(request.params.clone().unwrap_or_default());
                let Ok(params) = serde_json::from_value::<CallToolRequestParams>(request) else {
                    return Ok(MessageMcpResponse::error(McpError::new(
                        i32::from(ErrorCode::INVALID_PARAMS),
                        "invalid tools/call parameters",
                    )));
                };
                serde_json::to_value(call_tool(inner, &self.inner.grant, params).await)
            }
            "ping" => serde_json::to_value(EmptyObject {}),
            _ => {
                return Ok(MessageMcpResponse::error(McpError::new(
                    i32::from(ErrorCode::METHOD_NOT_FOUND),
                    "unsupported MCP method",
                )));
            }
        };
        result.map(MessageMcpResponse::success).map_err(Error::from)
    }

    /// Lets the thread's tools run as `run` until the guard drops, which also
    /// cancels calls still running.
    pub(super) fn activate(
        &self,
        run: RunId,
        cancel: CancellationToken,
        receipts: Arc<Receipts>,
    ) -> ActiveTurn {
        let grant = Arc::clone(&self.inner.grant);
        *grant.active.lock().expect("MCP grant lock poisoned") = Some(Active {
            run,
            cancel: cancel.clone(),
            receipts,
        });
        ActiveTurn { grant, cancel }
    }
}

/// Ends a turn's tool access on drop.
pub(super) struct ActiveTurn {
    grant: Arc<Grant>,
    cancel: CancellationToken,
}

impl Drop for ActiveTurn {
    fn drop(&mut self) {
        self.grant
            .active
            .lock()
            .expect("MCP grant lock poisoned")
            .take();
        self.cancel.cancel();
    }
}

async fn authenticate(
    State(inner): State<Weak<Inner>>,
    mut request: Request,
    next: Next,
) -> Response {
    let grant = inner.upgrade().and_then(|inner| {
        let token = bearer(request.headers())?;
        inner
            .grants
            .lock()
            .expect("MCP grants lock poisoned")
            .get(token)
            .cloned()
    });
    let Some(grant) = grant else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response();
    };
    request.extensions_mut().insert(grant);
    next.run(request).await
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

#[derive(Clone)]
struct Handler {
    tools: Arc<[Tool]>,
    inner: Weak<Inner>,
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        server_info()
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, ErrorData>> + Send {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools.to_vec())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let grant = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Arc<Grant>>())
            .cloned()
            .ok_or_else(|| ErrorData::invalid_request("Unauthenticated MCP request", None))?;
        let Some(inner) = self.inner.upgrade() else {
            return Ok(tools_unavailable().into());
        };
        let result = tokio::select! {
            result = call_tool(&inner, &grant, request) => result,
            () = context.ct.cancelled() => CallToolResult::error(vec![ContentBlock::text("The MCP request was cancelled")]),
        };
        Ok(result.into())
    }
}

async fn call_tool(inner: &Inner, grant: &Grant, request: CallToolRequestParams) -> CallToolResult {
    let active = {
        let active = grant.active.lock().expect("MCP grant lock poisoned");
        active.as_ref().map(|active| {
            (
                active.run,
                active.cancel.clone(),
                Arc::clone(&active.receipts),
            )
        })
    };
    let Some((run, cancel, receipts)) = active else {
        return CallToolResult::error(vec![ContentBlock::text("No active ACP turn")]);
    };
    let dispatch = inner.dispatch.get().and_then(Weak::upgrade);
    let Some(dispatch) = dispatch else {
        return tools_unavailable();
    };
    let name = request.name.into_owned();
    let args = Value::Object(request.arguments.unwrap_or_default());
    let scope = ToolScope {
        thread: grant.thread,
        run,
    };
    let result = tokio::select! {
        result = dispatch.call(scope, name.clone(), args) => result,
        () = cancel.cancelled() => Err("The ACP turn ended".to_owned()),
        () = inner.shutdown.cancelled() => Err("The MCP endpoint stopped".to_owned()),
    };
    let (text, child, child_environment, failed) = match result {
        Ok(ToolResult {
            text,
            child,
            child_environment,
        }) => (text, child, child_environment, false),
        Err(message) => (message, None, None, true),
    };
    let blocks = vec![
        ContentBlock::text(text),
        ContentBlock::text(receipts.issue(&name, child, child_environment)),
    ];
    if failed {
        CallToolResult::error(blocks)
    } else {
        CallToolResult::success(blocks)
    }
}

fn server_info() -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation::new("viz", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
}

fn tools_unavailable() -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text("viz tools are not available")])
}

impl std::fmt::Debug for Mcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mcp")
            .field("url", &self.inner.url)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for McpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpEndpoint")
            .field("url", &self.url())
            .finish_non_exhaustive()
    }
}
