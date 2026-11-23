//! Pure command-palette filtering and result-row derivations.

use viz_client::protocol::{SearchHit, ThreadSummary};

use super::{Model, PaletteChoice, PaletteCommand, thread_title};

const RECENT_LIMIT: usize = 6;
const CHAT_LIMIT: usize = 24;

/// All local commands in palette display order.
pub const PALETTE_COMMANDS: [PaletteCommand; 7] = [
    PaletteCommand::NewThread,
    PaletteCommand::AddProject,
    PaletteCommand::Sidebar,
    PaletteCommand::Pin,
    PaletteCommand::Archive,
    PaletteCommand::Delete,
    PaletteCommand::Stop,
];

impl PaletteCommand {
    /// Contextual display label for this command.
    #[must_use]
    pub fn label(self, model: &Model) -> &'static str {
        let thread = model.selection.selected.and_then(|id| model.summary(id));
        match self {
            Self::NewThread => "New thread",
            Self::AddProject => "Add project",
            Self::Sidebar => "Toggle sidebar",
            Self::Pin if thread.is_some_and(|thread| thread.lifecycle.pin.is_some()) => {
                "Unpin thread"
            }
            Self::Pin => "Pin thread",
            Self::Archive if thread.is_some_and(|thread| thread.lifecycle.archived) => {
                "Unarchive thread"
            }
            Self::Archive => "Archive thread",
            Self::Delete => "Delete thread",
            Self::Stop => "Stop response",
        }
    }

    /// Single-key command shortcut hint, when bound.
    #[must_use]
    pub const fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::NewThread => Some("N"),
            Self::Sidebar => Some("B"),
            Self::Stop => Some("."),
            _ => None,
        }
    }

    /// Whether this command can currently execute.
    #[must_use]
    pub fn available(self, model: &Model) -> bool {
        if self == Self::Sidebar {
            return true;
        }
        if !model.connected() {
            return false;
        }
        let thread = model.selection.selected.and_then(|id| model.summary(id));
        match self {
            Self::NewThread => model.selection.new_thread.is_none(),
            Self::AddProject | Self::Sidebar => true,
            Self::Pin | Self::Archive | Self::Delete => {
                model.selection.new_thread.is_none() && thread.is_some()
            }
            Self::Stop => model.selection.selected.is_some() && model.busy(),
        }
    }
}

/// One row derived for the command palette result list.
#[derive(Debug, Clone, Copy)]
pub enum PaletteRow<'a> {
    /// Local command row.
    Command(PaletteCommand),
    /// Existing thread row.
    Thread(&'a ThreadSummary),
    /// Transcript full-text search hit.
    Message(&'a SearchHit),
}

impl PaletteRow<'_> {
    /// Choice key emitted when this row is highlighted or activated.
    #[must_use]
    pub const fn choice(self) -> PaletteChoice {
        match self {
            Self::Command(command) => PaletteChoice::Command(command),
            Self::Thread(thread) => PaletteChoice::Thread(thread.id),
            Self::Message(hit) => PaletteChoice::Message {
                thread: hit.thread,
                entry: hit.entry.0,
            },
        }
    }

    /// Section heading for this row.
    #[must_use]
    pub const fn group(self, recent: bool) -> &'static str {
        match self {
            Self::Command(_) => "Commands",
            Self::Thread(_) if recent => "Recent threads",
            Self::Thread(_) => "Threads",
            Self::Message(_) => "Messages",
        }
    }
}

/// Case-insensitive multi-word substring filter.
#[must_use]
pub fn matches_filter(label: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if label.is_ascii() {
        filter.split_whitespace().all(|word| {
            label
                .as_bytes()
                .windows(word.len())
                .any(|part| part.eq_ignore_ascii_case(word.as_bytes()))
        })
    } else {
        let folded = label.to_lowercase();
        filter.split_whitespace().all(|word| folded.contains(word))
    }
}

/// Derives all visible palette rows for the current filter and connection.
#[must_use]
pub fn palette_rows(model: &Model, recent: bool) -> Vec<PaletteRow<'_>> {
    let filter = model.palette.filter.as_str();
    let connected = model.connected();
    let commands = PALETTE_COMMANDS
        .into_iter()
        .filter(|command| command.available(model) && matches_filter(command.label(model), filter))
        .map(PaletteRow::Command);
    let mut out: Vec<PaletteRow<'_>> = commands.collect();
    if connected {
        let limit = if recent { RECENT_LIMIT } else { CHAT_LIMIT };
        out.extend(
            model
                .cache()
                .threads()
                .filter(|thread| {
                    (!recent || !thread.lifecycle.archived)
                        && matches_filter(thread_title(thread), filter)
                })
                .take(limit)
                .map(PaletteRow::Thread),
        );
        out.extend(model.palette.hits().iter().map(PaletteRow::Message));
    }
    out
}
