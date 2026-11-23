//! Embedded terminal state and the protocol used to attach to an environment's
//! PTYs.
//!
//! The environment owns the live interpreter, which produces device replies
//! even with no viewers attached. Attachments receive a bounded native snapshot
//! followed by coalesced PTY bytes. Snapshots preserve alternate screens, modes
//! and partial escape sequences without replaying the original queries.
//! The desktop can search and select locally using the restored screen and
//! scrollback. Updates to one viewer's viewport never affect other viewers.

#[cfg(feature = "emulator")]
mod screen;
mod wire;

#[cfg(feature = "emulator")]
pub use screen::{EmulationError, LiveScreen, Viewer};
pub use wire::{CellSize, Change, Exit, Sequence, Size, Snapshot, TerminalId, Update};
