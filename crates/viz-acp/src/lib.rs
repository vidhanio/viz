//! Supervised ACP v1 clients, sessions, turn folding, and caller-supplied MCP
//! tools.
//!
//! RPCs use the SDK's v1 request, response, notification, and envelope types.
//! The internal process module documents the transport and shutdown contract.
//! [`Client`] exposes typed lifecycle commands. [`Session`] binds a single
//! session to a process, filters load history, and folds observed updates into
//! entries, tool patches, and a live tail. [`ClientEvents`] supplies the
//! caller's permission policy, typed response channels and optional host I/O.
//! Bound MCP tools share one turn scope across HTTP, stdio relay, and ACP.
//!
//! # Fallback decisions
//!
//! | Operation | Native path | Fallback |
//! | --- | --- | --- |
//! | Fork | `session/fork` when advertised | Create a new session and insert the caller's rendered transcript as context in the next prompt. |
//! | Restore | `session/resume`, then `session/load` with content replay filtered from the bound session | Create a new session with the same context handoff as fork. |
//! | Steer | Submit to `_session/steering` when advertised. A `promptRequired` response means the caller sends a normal prompt. | The caller can queue the new input and signal the current turn to cancel after open tools finish. |
//! | Title | `session/set_title` when advertised | Return a local title for the caller to persist. |
//! | Options | `session/options` when advertised | A prompt-free session reports options. Select the requested model there to discover dependent choices. Delete the disposable session when supported. |
//!
//! A missing method can trigger the same fallback as an absent capability.
//! Other errors propagate because the operation may already have taken effect.
//! The caller controls the replay text, including the fork point and context
//! budget. A handoff is included as historical context alongside the first new
//! user prompt. Interrupted prompts have no idempotency key and aren't retried.
//!
//! # Installed harnesses
//!
//! Probed with local harness credentials in isolated working directories. All
//! initialized, opened a new session, reported config options, completed a
//! read-and-shell turn, and returned `cancelled` on request. A dash means a
//! capability was absent or the interaction was not observed in these prompts.
//!
//! | Harness | Options RPC | Reopen after restart | Native fork | Native steer advertised | MCP advertised | Permission seen | Form seen | Host I/O seen |
//! | --- | --- | --- | --- | --- | --- | --- | --- | --- |
//! | claude-agent-acp 0.108.0 | yes | resume | yes | yes | HTTP, SSE, ACP | — | yes | — |
//! | codex-acp 2.7.0 | — | resume | yes | yes | HTTP | — | yes | — |
//! | agy 1.5.13 | — | resume | — | — | stdio | yes | yes | — |
//! | omp 19.0.1 | — | resume | yes | yes | HTTP, SSE | yes | yes | read, terminal |
//! | pi-acp 0.0.34 | — | load | — | — | stdio schema only | — | — | — |
//!
//! All five advertised images; all except pi advertised embedded resources.
//! None advertised audio or archive in the probe. Claude advertised title,
//! status, providers, auth status/logout, and system prompt override. Codex
//! advertised title, providers, logout, and system prompt append. agy sent
//! duplicate camel/snake capability fields, advertised logout and additional
//! directories, and described system prompt append in vendor metadata.
//! agy, Claude, and Codex advertised additional directories. The standard
//! `auth/status` succeeded on Claude. Codex returned method-not-found despite
//! its legacy `_meta.authStatus` marker. omp returned an internal error from
//! the unadvertised options RPC. Pi replays user, agent, and tool updates
//! during load, reports effort as both config options and legacy modes, and
//! accepts MCP declarations without connecting them. Claude's MCP-over-ACP
//! bridge asks for stateless MCP discovery before listing tools. A real tool
//! call succeeded with Claude over ACP and HTTP and with agy over stdio.
//! Claude also accepted a native steer during a running turn.
//!
//! Standard skill blocks and skill command metadata belong to the
//! [ACP v2 draft](https://github.com/agentclientprotocol/agent-client-protocol/commit/281608f).
//! This v1 client accepts skills presented as slash commands and preserves
//! recognized v1 command extensions. The native steering extension uses
//! [host-owned prompt fallback](https://github.com/agentclientprotocol/claude-agent-acp/blob/main/examples/steering.ts).
//! Rich reports remain on the library boundary until a caller maps them into
//! its own storage or UI.

pub mod bridge;
mod capabilities;
mod client;
mod config;
mod connection;
mod error;
mod failure;
mod housekeeping;
mod interaction;
mod lifecycle;
mod local_io;
pub mod mcp;
mod options;
mod probe;
mod process;
mod prompt;
mod receipts;
mod session;
mod settings;
mod steering;
mod tool;
mod transcript;
mod turn;

pub use agent_client_protocol::v1::schema::{self, StopReason};
pub use capabilities::Capabilities;
pub use client::Client;
pub use config::HarnessConfig;
pub use connection::UpdateSink;
pub use error::Error;
pub use failure::{TurnFailure, availability};
pub use housekeeping::{
    Housekeeping, LoadedTurn, SavedSession, housekeep, load_history, saved_sessions,
};
pub use interaction::{ClientEvents, ClientRequest, PermissionPolicy};
pub use lifecycle::{OpenSession, OpenedSession, SessionSetup};
pub use local_io::{FileAccess, LocalIo, TerminalAccess};
pub use mcp::Mcp;
pub use options::{Catalog, Desired, Selection};
pub use probe::{catalog, refresh};
pub use process::drain_processes;
pub use prompt::{StoredAttachment, build_prompt, context_blocks};
pub use receipts::Receipts;
pub use session::{Outcome, Prompt, Session, SessionSnapshot, stop_reason_name};
pub use settings::{Command, Skill};
pub use steering::{SteerOutcome, Steering};
pub use tool::ToolPatch;
pub use transcript::{Output, Transcript};
pub use viz_protocol::{
    Body, Compaction, CompactionStatus, LiveTurn, Media, Plan, PlanContent, PlanEntry,
    PlanPriority, PlanStatus, Segment, TextKind, ToolCall, TranscriptEntry as Entry, VizLink,
};

/// An unsolicited harness notification or a local terminal completion.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    Notification(Box<schema::AgentNotification>),

    /// Final output for a terminal after its process and output streams settle.
    Terminal {
        session: schema::SessionId,
        terminal: schema::TerminalId,
        output: Box<schema::TerminalOutputResponse>,
    },
}

/// One update and the wall-clock observation time supplied to the pure fold.
#[derive(Debug, Clone)]
pub struct ObservedUpdate {
    pub update: schema::SessionUpdate,
    pub observed_at: i64,
}
