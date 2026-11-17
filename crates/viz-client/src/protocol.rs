//! Protocol v1 types, mirroring `backend/src/protocol.ts` (the source of truth;
//! its JSON Schema is `protocol/schema.json`).

use serde::{Deserialize, Serialize};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

/// A pi-durable conversation id.
pub type ConversationId = u64;
/// A pi-durable submission id.
pub type SubmissionId = u64;

/// What kind of client is connecting; decides which notifications it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    /// Full UI (desktop, mobile): watches whole conversations.
    App,
    /// Connector (Discord, iMessage, ...): receives replies to its own inputs
    /// only.
    Connector,
    /// One-off command-line client.
    Cli,
}

/// Identifies a client in `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client kind.
    pub kind: ClientKind,
    /// Client name; for connectors, the name inputs' [`Origin::connector`] use.
    pub name: String,
}

/// Where an input came from, so its reply is routed back there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Connector name, matching its `hello` name.
    pub connector: String,
    /// Connector-specific channel/chat id the reply goes to.
    pub channel: String,
    /// Connector-specific thread id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// Connector-specific id of the message being answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Human-readable author, shown to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// `hello` result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hello {
    /// Protocol version the backend speaks.
    pub protocol: u32,
    /// Backend version.
    pub version: String,
    /// The orchestrator's conversation id.
    pub root: ConversationId,
}

/// `conversation.submit` params.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Submit {
    /// Target conversation; `None` is the orchestrator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationId>,
    /// The input text.
    pub text: String,
    /// Idempotency key: a retry with the same id returns the same submission.
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Where the input came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// How a submission settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettledStatus {
    /// Answered.
    Done,
    /// Settled without an answer (aborted, failed, or nothing to answer).
    Unanswered,
}

/// A notification from the backend.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Notification {
    /// The answer to an input this connector submitted.
    #[serde(rename = "reply")]
    Reply {
        /// The input's origin.
        origin: Origin,
        /// The conversation it was submitted to.
        conversation: ConversationId,
        /// The submission.
        submission: SubmissionId,
        /// How it settled.
        status: SettledStatus,
        /// The answer's text, if any.
        text: Option<String>,
    },
    /// A watched conversation's view: full `value` first, then Chord delta
    /// `ops`.
    #[serde(rename = "conversation.view")]
    ConversationView {
        /// The conversation.
        conversation: ConversationId,
        /// Full view (first revision).
        #[serde(default)]
        value: Option<serde_json::Value>,
        /// Delta operations against the previous revision.
        #[serde(default)]
        ops: Option<serde_json::Value>,
    },
}
