//! Caller-mediated ACP requests. Waiting for a user never blocks the reader.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    Error, JsonRpcMessage, JsonRpcResponse, RawJsonRpcId, RawJsonRpcMessage, RawJsonRpcParams,
    RawJsonRpcRequest,
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use viz_protocol::{FormValue, PermissionKind, PermissionOption, RequestAnswer, RequestBody};

use crate::{
    local_io::{Host, LocalIo, TerminalAccess},
    mcp::McpEndpoint,
    schema::{
        AgentRequest, BooleanConfigOptionCapabilities, ClientCapabilities,
        ClientSessionCapabilities, CreateElicitationRequest, CreateElicitationResponse,
        ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
        ElicitationContentValue, ElicitationFormCapabilities, ElicitationMode, ElicitationScope,
        MessageMcpRequest, MessageMcpResponse, PermissionOptionId, PermissionOptionKind,
        PlanCapabilities, PromptSuggestionsCapabilities, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
        SessionConfigOptionsCapabilities, SessionId, SubagentCapabilities,
    },
};

/// Whether the client answers permission requests itself or asks its caller.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    #[default]
    FullAccess,
    Ask,
}

/// Optional client-side services and the channel that receives their requests.
#[derive(Debug, Default, Clone)]
pub struct ClientEvents {
    pub permissions: PermissionPolicy,
    pub local_io: LocalIo,
    pub events: Option<mpsc::UnboundedSender<crate::ClientEvent>>,
    pub requests: Option<mpsc::UnboundedSender<ClientRequest>>,
}

/// A request from the harness with a typed, single-use response channel.
/// Dropping `reply` cancels the request.
#[derive(Debug)]
pub enum ClientRequest {
    /// An approval or a question for the caller to answer.
    Interaction {
        session: SessionId,
        body: RequestBody,
        reply: oneshot::Sender<RequestAnswer>,
    },
    Mcp {
        request: MessageMcpRequest,
        reply: oneshot::Sender<Result<MessageMcpResponse, Error>>,
    },
}

impl ClientEvents {
    #[must_use]
    pub fn capabilities(&self) -> ClientCapabilities {
        let caps = ClientCapabilities::new()
            .fs(self.local_io.file_capabilities())
            .terminal(!matches!(self.local_io.terminals, TerminalAccess::Disabled))
            .plan(PlanCapabilities::new())
            .subagents(SubagentCapabilities::new())
            .session(
                ClientSessionCapabilities::new()
                    .prompt_suggestions(PromptSuggestionsCapabilities::new())
                    .config_options(
                        SessionConfigOptionsCapabilities::new()
                            .boolean(BooleanConfigOptionCapabilities::new()),
                    ),
            );
        if self.requests.is_some() {
            caps.elicitation(
                ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
            )
        } else {
            caps
        }
    }
}

pub struct Incoming {
    events: ClientEvents,
    pub host: Host,
    endpoint: Option<McpEndpoint>,
    pending: Mutex<HashMap<RawJsonRpcId, Pending>>,
}

struct Pending {
    session: Option<SessionId>,
    cancel: CancellationToken,
}

impl Incoming {
    pub fn new(events: ClientEvents, host: Host, endpoint: Option<McpEndpoint>) -> Self {
        Self {
            events,
            host,
            endpoint,
            pending: Mutex::default(),
        }
    }

    pub fn prepare(self: &Arc<Self>, request: RawJsonRpcRequest) -> Job {
        let method = request.method;
        let params = request
            .params
            .map_or(Value::Null, RawJsonRpcParams::into_value);
        let work = Work::parse(&method, &params);
        let cancel = CancellationToken::new();
        if let Ok(work) = &work {
            let mut pending = self.pending.lock().expect("ACP incoming lock poisoned");
            if pending.contains_key(&request.id) {
                return Job {
                    incoming: Arc::clone(self),
                    id: request.id,
                    method,
                    work: Err(Error::invalid_request()),
                    cancel,
                    registered: false,
                };
            }
            pending.insert(
                request.id.clone(),
                Pending {
                    session: work.session(),
                    cancel: cancel.clone(),
                },
            );
        }
        let registered = work.is_ok();
        Job {
            incoming: Arc::clone(self),
            id: request.id,
            method,
            work,
            cancel,
            registered,
        }
    }

    pub fn cancel_session(&self, session: &str) {
        self.host.cancel_session(session);
        for pending in self
            .pending
            .lock()
            .expect("ACP incoming lock poisoned")
            .values()
        {
            if pending
                .session
                .as_ref()
                .is_some_and(|id| id.0.as_ref() == session)
            {
                pending.cancel.cancel();
            }
        }
    }

    pub fn cancel_request(&self, id: &RawJsonRpcId) {
        if let Some(pending) = self
            .pending
            .lock()
            .expect("ACP incoming lock poisoned")
            .get(id)
        {
            pending.cancel.cancel();
        }
    }
}

pub struct Job {
    incoming: Arc<Incoming>,
    id: RawJsonRpcId,
    method: Arc<str>,
    work: Result<Work, Error>,
    cancel: CancellationToken,
    registered: bool,
}

impl Job {
    pub async fn answer(self, closed: &CancellationToken) -> RawJsonRpcMessage {
        let result = match self.work {
            Ok(work) => tokio::select! {
                biased;
                () = closed.cancelled() => work.cancelled(&self.method),
                () = self.cancel.cancelled() => work.cancelled(&self.method),
                result = work.answer(&self.incoming, &self.method) => result,
            },
            Err(error) => Err(error),
        };
        if self.registered {
            self.incoming
                .pending
                .lock()
                .expect("ACP incoming lock poisoned")
                .remove(&self.id);
        }
        RawJsonRpcMessage::response(self.id, result)
    }
}

enum Work {
    Permission(Box<RequestPermissionRequest>),
    Elicitation(Box<CreateElicitationRequest>),
    Mcp(MessageMcpRequest),
    Host(Box<AgentRequest>),
}

impl Work {
    fn parse(method: &str, params: &Value) -> Result<Self, Error> {
        match AgentRequest::parse_message(method, params)? {
            AgentRequest::RequestPermissionRequest(request) => {
                Ok(Self::Permission(Box::new(request)))
            }
            AgentRequest::CreateElicitationRequest(request)
                if matches!(request.mode, ElicitationMode::Form(_)) =>
            {
                Ok(Self::Elicitation(Box::new(request)))
            }
            AgentRequest::MessageMcpRequest(request) => Ok(Self::Mcp(request)),
            request @ (AgentRequest::ReadTextFileRequest(_)
            | AgentRequest::WriteTextFileRequest(_)
            | AgentRequest::CreateTerminalRequest(_)
            | AgentRequest::TerminalOutputRequest(_)
            | AgentRequest::WaitForTerminalExitRequest(_)
            | AgentRequest::KillTerminalRequest(_)
            | AgentRequest::ReleaseTerminalRequest(_)) => Ok(Self::Host(Box::new(request))),
            _ => Err(Error::method_not_found()),
        }
    }

    fn session(&self) -> Option<SessionId> {
        match self {
            Self::Permission(request) => Some(request.session_id.clone()),
            Self::Elicitation(request) => match &request.mode {
                ElicitationMode::Form(form) => match &form.scope {
                    ElicitationScope::Session(scope) => Some(scope.session_id.clone()),
                    _ => None,
                },
                _ => None,
            },
            Self::Mcp(_) => None,
            Self::Host(request) => Some(host_session(request)?.clone()),
        }
    }

    fn cancelled(&self, method: &str) -> Result<Value, Error> {
        match self {
            Self::Permission(_) => permission_response(None).into_json(method),
            Self::Elicitation(_) => {
                CreateElicitationResponse::new(ElicitationAction::Cancel).into_json(method)
            }
            Self::Mcp(_) | Self::Host(_) => Err(Error::request_cancelled()),
        }
    }

    async fn answer(&self, incoming: &Incoming, method: &str) -> Result<Value, Error> {
        let events = &incoming.events;
        match self {
            Self::Permission(request) => {
                let choice = if events.permissions == PermissionPolicy::FullAccess {
                    auto_allow(request)
                } else {
                    ask(events, |reply| ClientRequest::Interaction {
                        session: request.session_id.clone(),
                        body: permission_body(request),
                        reply,
                    })
                    .await
                    .and_then(|answer| match answer {
                        RequestAnswer::Permission { option } => request
                            .options
                            .iter()
                            .find(|offered| *offered.option_id.0 == *option)
                            .map(|offered| offered.option_id.clone()),
                        RequestAnswer::Accept { .. } | RequestAnswer::Decline => None,
                    })
                };
                permission_response(choice).into_json(method)
            }
            Self::Elicitation(request) => {
                let ElicitationMode::Form(form) = &request.mode else {
                    return Err(Error::invalid_params());
                };
                let schema = serde_json::to_value(&form.requested_schema)
                    .map_err(Error::into_internal_error)?;
                let action = ask(events, |reply| ClientRequest::Interaction {
                    session: self.session().unwrap_or_else(|| SessionId::new("")),
                    body: RequestBody::Question {
                        message: request.message.clone(),
                        schema,
                    },
                    reply,
                })
                .await
                .map_or(ElicitationAction::Cancel, elicitation_action);
                CreateElicitationResponse::new(action).into_json(method)
            }
            Self::Mcp(request)
                if incoming
                    .endpoint
                    .as_ref()
                    .is_some_and(|endpoint| endpoint.owns_acp_request(request)) =>
            {
                let endpoint = incoming
                    .endpoint
                    .as_ref()
                    .ok_or_else(Error::method_not_found)?;
                endpoint.call_acp(request).await?.into_json(method)
            }
            Self::Mcp(request) => ask(events, |reply| ClientRequest::Mcp {
                request: request.clone(),
                reply,
            })
            .await
            .ok_or_else(Error::method_not_found)??
            .into_json(method),
            Self::Host(request) => incoming.host.handle(request, method).await,
        }
    }
}

async fn ask<T>(
    events: &ClientEvents,
    request: impl FnOnce(oneshot::Sender<T>) -> ClientRequest,
) -> Option<T> {
    let sender = events.requests.as_ref()?;
    let (reply, answer) = oneshot::channel();
    sender.send(request(reply)).ok()?;
    answer.await.ok()
}

fn permission_body(request: &RequestPermissionRequest) -> RequestBody {
    RequestBody::Permission {
        call: request.tool_call.tool_call_id.0.to_string(),
        title: request.tool_call.fields.title.clone().unwrap_or_default(),
        options: request
            .options
            .iter()
            .filter_map(|option| {
                Some(PermissionOption {
                    id: option.option_id.0.to_string(),
                    label: option.name.clone(),
                    kind: match option.kind {
                        PermissionOptionKind::AllowOnce => PermissionKind::AllowOnce,
                        PermissionOptionKind::AllowAlways => PermissionKind::AllowAlways,
                        PermissionOptionKind::RejectOnce => PermissionKind::RejectOnce,
                        PermissionOptionKind::RejectAlways => PermissionKind::RejectAlways,
                        _ => return None,
                    },
                })
            })
            .collect(),
    }
}

fn elicitation_action(answer: RequestAnswer) -> ElicitationAction {
    match answer {
        RequestAnswer::Accept { fields } => {
            let content: BTreeMap<_, _> = fields
                .into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        FormValue::Boolean(value) => ElicitationContentValue::Boolean(value),
                        FormValue::Number(number) => number.as_i64().map_or_else(
                            || ElicitationContentValue::Number(number.as_f64().unwrap_or_default()),
                            ElicitationContentValue::Integer,
                        ),
                        FormValue::Text(text) => ElicitationContentValue::String(text),
                        FormValue::Choices(choices) => {
                            ElicitationContentValue::StringArray(choices)
                        }
                    };
                    (key, value)
                })
                .collect();
            ElicitationAction::Accept(ElicitationAcceptAction::new().content(content))
        }
        RequestAnswer::Decline => ElicitationAction::Decline,
        RequestAnswer::Permission { .. } => ElicitationAction::Cancel,
    }
}

fn permission_response(choice: Option<PermissionOptionId>) -> RequestPermissionResponse {
    RequestPermissionResponse::new(choice.map_or(RequestPermissionOutcome::Cancelled, |id| {
        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
    }))
}

fn auto_allow(request: &RequestPermissionRequest) -> Option<PermissionOptionId> {
    [
        PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways,
    ]
    .iter()
    .find_map(|kind| request.options.iter().find(|option| &option.kind == kind))
    .map(|option| option.option_id.clone())
}

const fn host_session(request: &AgentRequest) -> Option<&SessionId> {
    match request {
        AgentRequest::ReadTextFileRequest(request) => Some(&request.session_id),
        AgentRequest::WriteTextFileRequest(request) => Some(&request.session_id),
        AgentRequest::CreateTerminalRequest(request) => Some(&request.session_id),
        AgentRequest::TerminalOutputRequest(request) => Some(&request.session_id),
        AgentRequest::WaitForTerminalExitRequest(request) => Some(&request.session_id),
        AgentRequest::KillTerminalRequest(request) => Some(&request.session_id),
        AgentRequest::ReleaseTerminalRequest(request) => Some(&request.session_id),
        _ => None,
    }
}
