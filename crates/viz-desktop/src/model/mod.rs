//! Pure domain and client state for `viz-desktop`.
//!
//! Nothing under `model` imports Rune, Tokio, channels, or performs I/O. Every
//! state container is plain data and every projection or derivation is a pure
//! function.

pub mod prefs;
pub mod tool_parse;
pub mod transcript;

pub use self::{
    prefs::{Prefs, SidebarMode},
    tool_parse::is_command,
    transcript::{Item, PlanStep, TaskResult, Tool, ToolStatus, Transcript, TurnEnd, User},
};
