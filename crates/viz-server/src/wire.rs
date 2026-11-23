//! Shared protocol types and the server's JSON-RPC envelope.

use jsonrpsee_types::{ErrorObject, Response as JsonResponse, ResponsePayload};
pub(crate) use jsonrpsee_types::{Id, Request};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
pub(crate) use viz_protocol::{
    Attachment, AttachmentCapabilities, ClientKind, ConfigOption, ContextUsage as Usage, Entry,
    EntryData as NewEntry, EntryId, EntryPage, Environment, EnvironmentStatus, Harness, Hello,
    HelloParams, LiveText, LiveTextKind, LiveTurn, Notification, OptionValue, Origin,
    PROTOCOL_VERSION, PlanEntry, PlanPriority, PlanStatus, Project, PromptAttachment, QueuedInput,
    Run, RunId, RunKind, RunStatus, SearchHit, SlashCommand as Command, Task, TaskStatus, ThreadId,
    ThreadState, ThreadSummary, ThreadWorktree, ToolCallStatus, ToolDiff, ToolRecord, WaitResult,
};

use crate::error::{Error, Kind, RequestError};

pub(crate) const RPC_PATH: &str = "/v2/rpc";
pub(crate) const HEALTH_PATH: &str = "/v2/health";

pub(crate) use viz_acp::StoredAttachment;

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

const fn git_failure(error: &Error) -> Option<viz_protocol::GitFailure> {
    use viz_git::{CommandFailure as Command, Error as Git};
    use viz_protocol::GitFailure as Failure;

    let Error::Git(error) = error else {
        return None;
    };
    Some(match error {
        Git::Busy => Failure::Busy,
        Git::Dirty => Failure::Dirty,
        Git::Conflicted => Failure::Conflicted,
        Git::Cancelled => Failure::Cancelled,
        Git::Bare | Git::NotRepository => Failure::NotRepository,
        Git::Invalid(_) => Failure::Invalid,
        Git::StaleCheckpoint => Failure::StaleCheckpoint,
        Git::CheckpointObstruction(_) => Failure::CheckpointObstruction,
        Git::Command { kind, .. } => match kind {
            Command::NonFastForward => Failure::NonFastForward,
            Command::Authentication => Failure::Authentication,
            Command::HostKey => Failure::HostKey,
            Command::HookRejected => Failure::HookRejected,
            Command::NothingToCommit => Failure::NothingToCommit,
            Command::MissingUpstream => Failure::MissingUpstream,
            Command::MissingIdentity => Failure::MissingIdentity,
            Command::BranchAlreadyExists => Failure::BranchAlreadyExists,
            Command::Other => Failure::Command,
        },
        _ => Failure::Unavailable,
    })
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
                git_failure(&error),
            )),
        };
        Self(JsonResponse::new(payload, id))
    }
}
