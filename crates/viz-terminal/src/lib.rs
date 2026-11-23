//! Embedded terminal state and the protocol used to attach to an environment's
//! PTYs.
//!
//! The environment owns the live interpreter, which produces device replies
//! even with no viewers attached. Attachments receive a bounded native snapshot
//! followed by coalesced PTY bytes. Snapshots preserve alternate screens, modes
//! and partial escape sequences without replaying the original queries.
//! The desktop can search and select locally using the restored screen and
//! scrollback. Updates to one viewer's viewport never affect other viewers.
//!
//! A local 100×30 PTY run of `seq 1 2000000` produced 16.9 MB of PTY bytes,
//! about 22.5 MB with JSON base64 before compression. Mid-stream attachment
//! restored 4,609 retained rows from a 49 KB snapshot in about 1.5 ms.
//! Streaming keeps native selection, focus, search and mode-dependent input
//! local. Reattach cost depends on retained state, and echo latency depends
//! on the round trip to the environment. A projected row stream could skip
//! intermediate rows under load, but would need a separate history protocol
//! to recover them.

#[cfg(all(unix, feature = "pty"))]
pub mod pty;
#[cfg(feature = "emulator")]
mod screen;

#[cfg(feature = "emulator")]
pub use screen::{EmulationError, LiveScreen, Viewer};
pub use viz_protocol::{
    CellSize, Sequence, Size, Snapshot, TerminalChange as Change, TerminalExit as Exit, TerminalId,
    Update,
};
