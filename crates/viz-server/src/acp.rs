//! The ACP runtime: harness processes, their sessions and turns, native session
//! import, option catalogs, and the MCP server that gives agents viz's tools.
//!
//! What viz can do with a harness is bounded by stable ACP v1
//! (`docs/findings/acp.md`):
//!
//! - Prompts carry no idempotency key, so a turn interrupted by a crash or
//!   restart is never replayed.
//! - There is no steering: steering a running turn cancels it
//!   (`session/cancel`) and queues the new prompt.
//! - Reopening a saved session needs `session/resume` or `session/load`;
//!   `session/load`'s history replay is discarded.
//! - Importing a native session needs both `session/list` and `session/load`.
//!   The replay carries no stop reasons and only text prompt content survives.
//!   A session stays bound to the directory it was created or imported in.
//! - Model-dependent effort choices are learned by probing a disposable
//!   session, since stable ACP cannot ask about an inactive model.
//! - viz advertises no `fs` or `terminal` client capabilities and allows every
//!   permission request.
//! - Runners get viz's MCP server through ACP `mcpServers` only; a runner that
//!   ignores it cannot use viz's tools.

pub(crate) mod bridge;
pub(crate) mod mcp;
pub(crate) mod options;
pub(crate) mod probe;
pub(crate) mod process;
pub(crate) mod session;
pub(crate) mod tool_record;
mod transcript;

pub(crate) use agent_client_protocol::v1::schema::StopReason;
pub(crate) use mcp::Mcp;
pub(crate) use options::{Catalog, Desired, Selection};
pub(crate) use probe::{catalog, list};
pub(crate) use process::drain_processes;
pub(crate) use session::{Outcome, Prompt, Replay, Session, SessionSnapshot, stop_reason_name};
