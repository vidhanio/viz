//! The ACP runtime: harness processes, their sessions and turns, option
//! catalogs, and the MCP server that gives agents viz's tools.
//!
//! viz speaks ACP v1 and uses an unstable method only where the harness
//! advertises it, doing the same job itself otherwise:
//!
//! - Prompts carry no idempotency key, so a turn interrupted by a crash or
//!   restart is never replayed.
//! - v1 has no in-turn input: steering waits for open tool calls to finish,
//!   cancels the turn (`session/cancel`), and sends the new prompt as the next
//!   run.
//! - Reopening a saved session needs `session/resume` or `session/load`;
//!   `session/load`'s history replay is discarded. A session stays bound to the
//!   directory it was created in. An idle session the harness can reopen has
//!   its process stopped, as on a restart.
//! - Forking copies the source conversation with `session/fork`; otherwise the
//!   fork opens a new session whose first prompt carries a bounded transcript.
//! - Option catalogs come from `session/options`, or from a disposable
//!   prompt-free session. Effort choices of an inactive model always need one.
//! - viz advertises only boolean config options among the client capabilities,
//!   and allows every permission request that offers an allow option.
//! - Runners get viz's MCP server through ACP `mcpServers` only; a runner that
//!   ignores it cannot use viz's tools.

pub(crate) mod bridge;
pub(crate) mod mcp;
pub(crate) mod options;
pub(crate) mod probe;
pub(crate) mod process;
pub(crate) mod session;
pub(crate) mod tool_record;
pub(crate) mod transcript;

pub(crate) use agent_client_protocol::v1::schema::StopReason;
pub(crate) use mcp::Mcp;
pub(crate) use options::{Catalog, Desired, Selection};
pub(crate) use probe::catalog;
pub(crate) use process::drain_processes;
pub(crate) use session::{
    Opened, Origin, Outcome, Prompt, Session, SessionSnapshot, stop_reason_name,
};
