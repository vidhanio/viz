//! Terminal attachment and stream messages.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Environment-scoped terminal identifier.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct TerminalId(pub Uuid);

/// Position in a terminal's update stream. Updates are contiguous; a gap needs
/// a fresh [`Snapshot`].
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct Sequence(pub u64);

impl Sequence {
    /// The sequence of the update after this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// A grid size in cells, clamped to what the emulator hosts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(from = "(u16, u16)", into = "(u16, u16)")]
#[schemars(with = "(u16, u16)")]
pub struct Size {
    columns: u16,
    rows: u16,
}

impl Size {
    /// Widest supported grid.
    pub const MAX_COLUMNS: u16 = 512;
    /// Tallest supported grid.
    pub const MAX_ROWS: u16 = 256;

    /// Clamps the grid to the supported range.
    #[must_use]
    pub fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns: columns.clamp(2, Self::MAX_COLUMNS),
            rows: rows.clamp(1, Self::MAX_ROWS),
        }
    }

    /// The number of columns.
    #[must_use]
    pub const fn columns(self) -> u16 {
        self.columns
    }

    /// The number of rows.
    #[must_use]
    pub const fn rows(self) -> u16 {
        self.rows
    }
}

impl Default for Size {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl From<(u16, u16)> for Size {
    fn from((columns, rows): (u16, u16)) -> Self {
        Self::new(columns, rows)
    }
}

impl From<Size> for (u16, u16) {
    fn from(size: Size) -> Self {
        (size.columns, size.rows)
    }
}

/// How a terminal process ended.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    /// The program returned this code.
    Code(i32),
    /// This signal ended the program.
    Signal(i32),
    /// The environment could not observe how the program ended.
    Lost,
}

/// Physical cell dimensions used for terminal size queries and mouse reporting.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CellSize {
    /// Pixel width of a cell.
    pub width: u16,
    /// Pixel height of a cell.
    pub height: u16,
}

/// Terminal state at an attachment boundary.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Snapshot {
    /// Last update already included in the state.
    pub sequence: Sequence,
    /// Complete libghostty snapshot including retained history and parser
    /// continuation.
    #[serde(with = "binary")]
    #[schemars(with = "String")]
    pub state: Vec<u8>,
    /// The process's status if it exited before this snapshot.
    pub exit: Option<Exit>,
}

/// The next update after a snapshot or earlier update.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct Update {
    /// Position in the stream.
    pub sequence: Sequence,
    /// Output or lifecycle change.
    pub change: Change,
}

/// Changes apply in PTY order, including changes to its size.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    /// Coalesced raw bytes from the PTY.
    Output {
        /// PTY bytes, represented as base64 in JSON.
        #[serde(with = "binary")]
        #[schemars(with = "String")]
        bytes: Vec<u8>,
    },
    /// The owner changed the PTY dimensions.
    Resized {
        /// Grid dimensions.
        size: Size,
        /// Pixel dimensions of a cell.
        cell: CellSize,
    },
    /// The process exited and the terminal remains readable.
    Exited {
        /// How the process ended.
        exit: Exit,
    },
}

mod binary {
    use base64::prelude::{BASE64_STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&base64::display::Base64Display::new(
            bytes,
            &BASE64_STANDARD,
        ))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        BASE64_STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(de::Error::custom)
    }
}
