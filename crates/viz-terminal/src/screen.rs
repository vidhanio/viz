//! Ghostty integration for the live PTY owner and query-free viewers.
//!
//! Ghostty ignores clipboard writes and other effects unless a handler is
//! registered, so only the live screen installs one: PTY replies.

use std::{cell::RefCell, rc::Rc};

use libghostty_vt::{Terminal, snapshot};

use crate::{CellSize, Change, Exit, Sequence, Size, Snapshot, Update};

/// Ghostty retains history at page granularity up to these limits.
const HISTORY_BYTES: usize = 8 * 1024 * 1024;
const HISTORY_LINES: usize = 5_000;

/// Bound parser buffers and the continuation stored in attachment snapshots.
const ESCAPE_BYTES: usize = 64 * 1024;

/// Limit the input processed before draining effects from the interpreter.
const INPUT_CHUNK_BYTES: usize = 16 * 1024;

/// A terminal operation failed or the viewer needs a fresh attachment.
#[derive(Debug, thiserror::Error)]
pub enum EmulationError {
    /// The native terminal could not complete the operation.
    #[error("terminal emulation failed")]
    Native(#[from] libghostty_vt::Error),
    /// A missed or repeated update broke the viewer's sequence.
    #[error("terminal update sequence mismatch (expected {expected:?}, got {received:?})")]
    StreamGap {
        /// Sequence after the last applied update.
        expected: Sequence,
        /// The update's sequence.
        received: Sequence,
    },
}

/// The PTY owner's interpreter. Query responses are collected while input is
/// interpreted and delivered to the caller's writer before it returns.
#[derive(Debug)]
pub struct LiveScreen {
    terminal: Terminal<'static, 'static>,
    replies: Rc<RefCell<Vec<u8>>>,
    sequence: Sequence,
    exit: Option<Exit>,
}

impl LiveScreen {
    /// Creates an interpreter with bounded scrollback and parser buffers.
    ///
    /// # Errors
    /// Returns an error if Ghostty cannot allocate or configure the
    /// interpreter.
    pub fn new(size: Size, cell: CellSize) -> Result<Self, EmulationError> {
        let mut terminal = Terminal::new(size.columns(), size.rows())?;
        let replies = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&replies);
        terminal.on_pty_write(move |_, bytes| sink.borrow_mut().extend_from_slice(bytes))?;
        configure(&mut terminal)?;
        resize(&mut terminal, size, cell)?;
        replies.borrow_mut().clear();
        Ok(Self {
            terminal,
            replies,
            sequence: Sequence::default(),
            exit: None,
        })
    }

    /// Interprets a batch and emits query responses for delivery to the live
    /// PTY.
    pub fn output(&mut self, bytes: Vec<u8>, mut reply: impl FnMut(&[u8])) -> Update {
        for chunk in bytes.chunks(INPUT_CHUNK_BYTES) {
            self.terminal.vt_write(chunk);
            self.deliver(&mut reply);
        }
        self.update(Change::Output { bytes })
    }

    /// Applies new grid dimensions and delivers any resulting size report.
    ///
    /// # Errors
    /// Returns an error if Ghostty cannot resize the grid.
    pub fn resize(
        &mut self,
        size: Size,
        cell: CellSize,
        mut reply: impl FnMut(&[u8]),
    ) -> Result<Update, EmulationError> {
        resize(&mut self.terminal, size, cell)?;
        self.deliver(&mut reply);
        Ok(self.update(Change::Resized { size, cell }))
    }

    /// Captures the state and the sequence of the last included update.
    ///
    /// # Errors
    /// Returns an error if Ghostty cannot encode the current state.
    pub fn snapshot(&self) -> Result<Snapshot, EmulationError> {
        Ok(Snapshot {
            sequence: self.sequence,
            state: self
                .terminal
                .encode_snapshot_alloc(None)?
                .map_or_else(Vec::new, |state| state.to_vec()),
            exit: self.exit,
        })
    }

    /// Records the process's exit while preserving the final terminal state.
    pub const fn exit(&mut self, exit: Exit) -> Update {
        self.exit = Some(exit);
        self.update(Change::Exited { exit })
    }

    fn deliver(&self, reply: &mut impl FnMut(&[u8])) {
        let replies = self.replies.take();
        if !replies.is_empty() {
            reply(&replies);
        }
    }

    const fn update(&mut self, change: Change) -> Update {
        self.sequence = self.sequence.next();
        Update {
            sequence: self.sequence,
            change,
        }
    }
}

/// A viewer restores an attachment snapshot and consumes updates in order.
///
/// Viewport, search, selection and input encoders can use the viewer's
/// interpreter. It registers no effect handlers, so applying updates exposes
/// no route for device replies.
#[derive(Debug)]
pub struct Viewer {
    terminal: Terminal<'static, 'static>,
    sequence: Sequence,
    exit: Option<Exit>,
}

impl Viewer {
    /// Restores terminal state produced by the owning environment.
    ///
    /// # Errors
    /// Returns an error if the snapshot is invalid or Ghostty cannot restore
    /// it.
    pub fn attach(snapshot: &Snapshot) -> Result<Self, EmulationError> {
        let mut decoder = snapshot::Decoder::new_buf(&snapshot.state)?;
        decoder.set_max_continuation_bytes(ESCAPE_BYTES)?;
        let mut terminal = decoder.decode()?;
        configure(&mut terminal)?;
        Ok(Self {
            terminal,
            sequence: snapshot.sequence,
            exit: snapshot.exit,
        })
    }

    /// Applies an update after checking its sequence.
    ///
    /// # Errors
    /// Returns an error on a sequence gap or if Ghostty cannot resize the grid.
    pub fn apply(&mut self, update: &Update) -> Result<(), EmulationError> {
        let expected = self.sequence.next();
        if update.sequence != expected {
            return Err(EmulationError::StreamGap {
                expected,
                received: update.sequence,
            });
        }
        match &update.change {
            Change::Output { bytes } => self.terminal.vt_write(bytes),
            Change::Resized { size, cell } => resize(&mut self.terminal, *size, *cell)?,
            Change::Exited { exit } => self.exit = Some(*exit),
        }
        self.sequence = update.sequence;
        Ok(())
    }

    /// Provides state for rendering and input encoders.
    #[must_use]
    pub const fn terminal(&self) -> &Terminal<'static, 'static> {
        &self.terminal
    }

    /// Provides local state for viewport, palette and selection changes.
    pub const fn terminal_mut(&mut self) -> &mut Terminal<'static, 'static> {
        &mut self.terminal
    }

    /// The exit status once the process finishes.
    #[must_use]
    pub const fn exit(&self) -> Option<Exit> {
        self.exit
    }
}

fn configure(terminal: &mut Terminal) -> Result<(), libghostty_vt::Error> {
    terminal.set_scrollback_max_bytes(Some(HISTORY_BYTES))?;
    terminal.set_scrollback_max_lines(Some(HISTORY_LINES))?;
    terminal.set_apc_max_bytes(Some(ESCAPE_BYTES))?;
    terminal.set_continuation_max_bytes(ESCAPE_BYTES)?;
    Ok(())
}

fn resize(terminal: &mut Terminal, size: Size, cell: CellSize) -> Result<(), libghostty_vt::Error> {
    terminal.resize(
        size.columns(),
        size.rows(),
        u32::from(cell.width),
        u32::from(cell.height),
    )
}
