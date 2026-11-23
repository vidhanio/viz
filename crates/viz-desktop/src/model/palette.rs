//! Pure command-palette filtering and result-row derivations.

use viz_client::protocol::{Project, SearchHit, ThreadSummary};

use super::{Model, PaletteChoice, PaletteCommand, thread_title};

const RECENT_LIMIT: usize = 12;
const CHAT_LIMIT: usize = 24;

/// All local commands in palette display order.
pub const PALETTE_COMMANDS: [PaletteCommand; 15] = [
    PaletteCommand::NewThread,
    PaletteCommand::AddProject,
    PaletteCommand::Projects,
    PaletteCommand::Settings,
    PaletteCommand::Import,
    PaletteCommand::Promote,
    PaletteCommand::Environments,
    PaletteCommand::Diff,
    PaletteCommand::Terminal,
    PaletteCommand::Scratch,
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
            Self::Projects => "Browse projects",
            Self::Settings => "Project settings",
            Self::Import => "Import harness session",
            Self::Promote => "Turn scratch thread into project",
            Self::Environments => "Switch environment",
            Self::Diff => "Toggle diff",
            Self::Terminal => "Toggle terminal",
            Self::Scratch => "New thread without a project",
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
            Self::AddProject | Self::Sidebar | Self::Diff | Self::Terminal | Self::Scratch => true,
            Self::Settings => model.project().is_some(),
            Self::Projects => model.cache().has_projects(),
            Self::Import => !model.harnesses().is_empty(),
            Self::Promote => thread.is_some_and(|thread| thread.project.is_none()),
            Self::Environments => model.connection.environments.len() > 1,
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
    Project(usize, &'a Project),
}

impl PaletteRow<'_> {
    /// Choice key emitted when this row is highlighted or activated.
    #[must_use]
    pub const fn choice(self) -> PaletteChoice {
        match self {
            Self::Project(index, _) => PaletteChoice::Project(index),
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
            Self::Message(_) => "In messages",
            Self::Project(..) => "Projects",
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

/// Reward exact and prefix matches ahead of substrings and ordered characters.
fn rank(label: &str, filter: &str) -> Option<u8> {
    let label = label.to_lowercase();
    if filter.is_empty() || label == filter {
        return Some(0);
    }
    if label.starts_with(filter) {
        return Some(1);
    }
    if matches_filter(&label, filter) {
        return Some(2);
    }
    let mut chars = label.chars();
    filter
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .all(|expected| chars.any(|ch| ch == expected))
        .then_some(3)
}

/// Derives all visible palette rows for the current filter and connection.
pub fn palette_rows(model: &Model, recent: bool) -> Vec<PaletteRow<'_>> {
    let raw = model.palette.filter.as_str();
    let actions_only = raw.starts_with('>');
    let filter = raw.strip_prefix('>').unwrap_or(raw).trim_start();
    let mut commands: Vec<_> = PALETTE_COMMANDS
        .into_iter()
        .filter(|command| command.available(model))
        .filter_map(|command| {
            Some((
                rank(command.label(model), filter)?,
                PaletteRow::Command(command),
            ))
        })
        .collect();
    commands.sort_by_key(|(score, _)| *score);
    let mut rows: Vec<_> = commands.into_iter().map(|(_, row)| row).collect();
    if model.connected() && !actions_only {
        let mut threads: Vec<_> = model
            .cache()
            .threads()
            .filter(|thread| !recent || !thread.lifecycle.archived)
            .filter_map(|thread| {
                let title = thread_title(thread);
                let path = thread.project.as_deref().unwrap_or_default();
                let project = model.cache().project(path);
                let name = project.map_or("", |project| project.name.as_str());
                let branch = thread
                    .worktree
                    .as_ref()
                    .and_then(|checkout| checkout.branch.as_deref())
                    .or_else(|| project.and_then(|project| project.branch.as_deref()))
                    .unwrap_or_default();
                let context = format!("{title} {name} {branch} {path}");
                Some((
                    rank(title, filter).or_else(|| rank(&context, filter))?,
                    PaletteRow::Thread(thread),
                ))
            })
            .collect();
        threads.sort_by_key(|(score, row)| {
            let updated_at = match row {
                PaletteRow::Thread(thread) => thread.updated_at,
                _ => 0,
            };
            (*score, std::cmp::Reverse(updated_at))
        });
        rows.extend(
            threads
                .into_iter()
                .take(if recent { RECENT_LIMIT } else { CHAT_LIMIT })
                .map(|(_, row)| row),
        );
        let mut projects: Vec<_> = model
            .cache()
            .projects()
            .enumerate()
            .filter_map(|(index, project)| {
                Some((
                    rank(&project.name, filter).or_else(|| rank(&project.path, filter))?,
                    PaletteRow::Project(index, project),
                ))
            })
            .collect();
        projects.sort_by_key(|(score, _)| *score);
        rows.extend(projects.into_iter().map(|(_, row)| row));
        rows.extend(model.palette.hits().iter().map(PaletteRow::Message));
    }
    rows
}
