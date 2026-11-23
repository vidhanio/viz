//! Approvals and questions a harness asks during a run, and the access mode
//! that decides which of them reach a person.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RequestId, RunId, ThreadId};

/// A durable request issued by a harness while a run is under way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub id: RequestId,
    pub thread: ThreadId,
    pub run: RunId,
    /// Unix milliseconds.
    pub created_at: i64,
    pub body: RequestBody,
    pub state: RequestState,
}

/// What the harness asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestBody {
    /// Approve a tool call. `call` is the transcript's tool call id.
    Permission {
        call: String,
        title: String,
        options: Vec<PermissionOption>,
    },
    /// Fill in a form, or decline to.
    Question {
        message: String,
        /// A JSON Schema object whose properties are primitive fields.
        schema: Value,
    },
}

/// One answer the harness accepts for a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionOption {
    pub id: String,
    pub label: String,
    pub kind: PermissionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

/// A reply to a pending request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestAnswer {
    /// Choose one of a permission request's options.
    Permission { option: String },
    /// Answer a question with values for its fields.
    Accept { fields: BTreeMap<String, FormValue> },
    /// Refuse to answer a question.
    Decline,
}

/// A form field's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FormValue {
    Boolean(bool),
    Number(serde_json::Number),
    Text(String),
    Choices(Vec<String>),
}

/// Who answered a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "thread", rename_all = "snake_case")]
pub enum AnsweredBy {
    /// A person, through a client.
    User,
    /// The agent of the parent thread that delegated this one.
    Agent(ThreadId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RequestState {
    Pending,
    Answered {
        at: i64,
        by: AnsweredBy,
        answer: RequestAnswer,
    },
    Withdrawn {
        at: i64,
        reason: Withdrawal,
    },
}

/// Why a request stopped waiting without an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Withdrawal {
    /// The run was stopped.
    Cancelled,
    /// New input steered the run.
    Steered,
    /// The harness withdrew the request or exited.
    Harness,
    /// The run ended.
    TurnEnded,
    /// The server stopped or restarted while the request waited.
    Restarted,
}

/// `thread.answer`: resolve one of a thread's pending requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnswerRequest {
    pub thread: ThreadId,
    pub request: RequestId,
    pub answer: RequestAnswer,
}

/// The broad behaviour of an access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessKind {
    /// Every permission request is approved without asking.
    Full,
    /// The harness decides most actions itself and asks about the rest.
    Auto,
    /// File edits are approved; other actions ask.
    AcceptEdits,
    /// Actions ask before they run.
    Ask,
    /// Read only, or planning without changes.
    ReadOnly,
}

/// One access mode a thread can run in: a harness's own mode where it has
/// one, or viz answering permission requests itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessMode {
    pub kind: AccessKind,
    /// The harness's label, or viz's for its own modes.
    pub label: String,
    /// The harness option value selecting this mode; `None` for a mode viz
    /// provides.
    pub native: Option<NativeAccess>,
}

/// A harness configuration option value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeAccess {
    pub option: String,
    pub value: String,
}

/// A thread's access mode, the choices, and where the mode came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Access {
    pub current: AccessMode,
    pub source: AccessSource,
    pub modes: Vec<AccessMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccessSource {
    /// Nothing was chosen; viz approves requests as it always has.
    Default,
    /// The project's default for this harness.
    Project,
    /// Chosen for this thread.
    Thread,
}

/// `thread.access`: choose a thread's access mode; `None` returns it to the
/// project default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetThreadAccess {
    pub thread: ThreadId,
    pub access: Option<AccessMode>,
}

/// `project.access`: a project's default access mode for one harness; `None`
/// clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetProjectAccess {
    pub project: String,
    pub harness: String,
    pub access: Option<AccessMode>,
}
