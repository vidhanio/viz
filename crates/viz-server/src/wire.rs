//! Shared protocol types and the server's JSON-RPC envelope.

use std::path::PathBuf;

use jsonrpsee_types::{ErrorObject, Response as JsonResponse, ResponsePayload};
pub(crate) use jsonrpsee_types::{Id, Request};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
pub(crate) use viz_protocol::{
    Attachment, AttachmentCapabilities, ClientKind, ConfigOption, ContextUsage as Usage,
    DependentChoice, DependentOption, Entry, EntryData as NewEntry, EntryId, EntryPage,
    Environment, EnvironmentStatus, Harness, Hello, HelloParams, LiveText, LiveTextKind, LiveTurn,
    Notification, OptionChoice, OptionKind, OptionValue, Origin, PROTOCOL_VERSION, PlanEntry,
    PlanPriority, PlanStatus, Project, PromptAttachment, QueuedInput, Run, RunId, RunKind,
    RunStatus, SearchHit, SlashCommand as Command, Task, TaskStatus, ThreadId, ThreadState,
    ThreadStatus, ThreadSummary, ThreadWorktree, ToolCallStatus, ToolDiff, ToolRecord, WaitResult,
};

use crate::error::{Error, Kind, RequestError};

pub(crate) const RPC_PATH: &str = "/v2/rpc";
pub(crate) const HEALTH_PATH: &str = "/v2/health";

#[derive(Debug, Clone)]
pub(crate) struct StoredAttachment {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mime: String,
    pub(crate) path: PathBuf,
}

const INVALID_PARAMS: i32 = -32602;
const NOT_FOUND: i32 = -32001;
const UNAVAILABLE: i32 = -32002;
const INTERNAL: i32 = -32603;

/// Keep the peer's code and message while classifying what its answer means.
pub(crate) fn peer_error(code: i32, message: &str) -> Error {
    Error::Peer {
        kind: match code {
            INVALID_PARAMS => Kind::Invalid,
            NOT_FOUND => Kind::NotFound,
            UNAVAILABLE => Kind::Unavailable,
            _ => Kind::Internal,
        },
        code,
        message: message.to_owned(),
    }
}

const fn error_code(error: &Error) -> i32 {
    match error {
        Error::Peer { code, .. } => *code,
        Error::Request(request) => match request {
            RequestError::Malformed(_) => -32700,
            RequestError::NotARequest(_) => -32600,
            RequestError::UnknownMethod(_) => -32601,
            RequestError::HelloRequired => -32000,
        },
        error => match error.kind() {
            Kind::Invalid => INVALID_PARAMS,
            Kind::NotFound => NOT_FOUND,
            Kind::Unavailable => UNAVAILABLE,
            Kind::Internal => INTERNAL,
        },
    }
}

pub(crate) fn decode<T: DeserializeOwned>(params: Value) -> Result<T, Error> {
    serde_json::from_value(params).map_err(|error| Error::invalid(error.to_string()))
}

pub(crate) fn encode(value: impl Serialize) -> Result<Value, Error> {
    Ok(serde_json::to_value(value)?)
}

pub(crate) fn raw_params<'a>(request: &'a Request<'_>) -> &'a str {
    request
        .params
        .as_deref()
        .map_or("{}", serde_json::value::RawValue::get)
}

pub(crate) fn params<T: DeserializeOwned>(request: &Request<'_>) -> Result<T, Error> {
    crate::rpc::parse(raw_params(request))
}

#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct Response<'a, T: Clone>(JsonResponse<'a, T>);

impl<'a, T: Clone> Response<'a, T> {
    pub(crate) fn new(id: Id<'a>, result: Result<T, Error>) -> Self {
        let payload = match result {
            Ok(result) => ResponsePayload::success(result),
            Err(error) => ResponsePayload::error(ErrorObject::owned(
                error_code(&error),
                error.to_string(),
                None::<()>,
            )),
        };
        Self(JsonResponse::new(payload, id))
    }
}
