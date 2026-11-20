//! Shared protocol types and the backend's JSON-RPC envelope.

use std::{fmt::Display, path::PathBuf};

use jsonrpsee_types::{ErrorObject, Response as JsonResponse, ResponsePayload};
pub(crate) use jsonrpsee_types::{Id, Request};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub(crate) use viz_client::protocol::{
    Attachment, AttachmentCapabilities, ClientKind, ConfigOption, ContextUsage as Usage,
    DependentChoice, DependentOption, Entry, EntryData as NewEntry, EntryId, EntryPage,
    Environment, EnvironmentStatus, Harness, HarnessThread as NativeThread,
    HarnessThreadList as NativeThreads, Hello, HelloParams, LiveText, LiveTextKind, LiveTurn,
    Notification, OptionChoice, OptionKind, OptionValue, Origin, PROTOCOL_VERSION, PlanEntry,
    PlanPriority, PlanStatus, Project, PromptAttachment, QueuedInput, Run, RunEnd, RunId,
    RunStatus, SearchHit, SlashCommand as Command, Task, TaskStatus, ThreadId, ThreadState,
    ThreadStatus, ThreadSummary, ToolCallStatus, ToolDiff, ToolRecord, WaitResult,
};

pub(crate) const RPC_PATH: &str = "/v2/rpc";
pub(crate) const HEALTH_PATH: &str = "/v2/health";
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const NOT_FOUND: i64 = -32001;
pub(crate) const UNAVAILABLE: i64 = -32002;

#[derive(Debug, Clone)]
pub(crate) struct StoredAttachment {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) mime: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(NOT_FOUND, message)
    }

    pub(crate) fn unavailable(message: impl Into<String>) -> Self {
        Self::new(UNAVAILABLE, message)
    }

    pub(crate) fn internal(error: impl Display) -> Self {
        Self::new(-32603, error.to_string())
    }
}

impl From<anyhow::Error> for RpcError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(rpc) = error.downcast_ref::<Self>() {
            return Self::new(rpc.code, rpc.message.clone());
        }
        Self::internal(error)
    }
}

impl From<toasty::Error> for RpcError {
    fn from(error: toasty::Error) -> Self {
        Self::internal(error)
    }
}

pub(crate) fn decode<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|error| RpcError::invalid(error.to_string()))
}

pub(crate) fn encode(value: impl Serialize) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(RpcError::internal)
}

pub(crate) fn params<T: DeserializeOwned>(request: &Request<'_>) -> Result<T, RpcError> {
    serde_json::from_str(
        request
            .params
            .as_deref()
            .map_or("{}", serde_json::value::RawValue::get),
    )
    .map_err(|error| RpcError::invalid(error.to_string()))
}

#[derive(Serialize)]
#[serde(transparent)]
pub(crate) struct Response<'a, T: Clone>(JsonResponse<'a, T>);

impl<'a, T: Clone> Response<'a, T> {
    pub(crate) fn new(id: Id<'a>, result: Result<T, RpcError>) -> Self {
        let payload = match result {
            Ok(result) => ResponsePayload::success(result),
            Err(error) => ResponsePayload::error(ErrorObject::owned(
                i32::try_from(error.code).unwrap_or(-32603),
                error.message,
                None::<()>,
            )),
        };
        Self(JsonResponse::new(payload, id))
    }
}
