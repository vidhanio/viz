//! Pure inline composer completion types, token parsing, and choice mapping
//! for `@` mentions, `/` commands, `$` skills, and `#` GitHub items.

use std::ops::Range;

use viz_client::protocol::{FileSearchKind, FileSearchMatch, GitHubItem, SlashCommand, ThreadId};

pub use super::completion_match::{
    matching_commands, matching_github, matching_mentions, matching_skills,
};
use super::{CompletionKind, CompletionToken, DraftCursor, DraftPill, Model};

/// Built-in client-side slash action executed immediately on acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCommand {
    /// Open the Model Picker.
    Model,
    /// Start a fresh session in the current project.
    Clear,
    /// Compact or summarize the conversation.
    Compact,
}

/// One selectable item in the tucked `/` slash-command drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashEntry {
    /// Command name without the leading `/`.
    pub name: String,
    /// Single-line description shown beside the command name.
    pub description: String,
    /// Optional argument hint.
    pub hint: Option<String>,
    /// Built-in desktop action, if not forwarded directly to the harness.
    pub builtin: Option<BuiltinCommand>,
    /// Ranking score (lower is better).
    pub score: u32,
}

/// One selectable item in the tucked `@` mention drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionEntry {
    /// Workspace file or directory from `files.search`.
    File(FileSearchMatch),
    /// Recent thread in the active environment.
    Thread {
        /// Thread identifier.
        id: ThreadId,
        /// Display title.
        title: String,
        /// Optional project name subtitle.
        project: Option<String>,
    },
    /// Configured viz environment peer.
    Environment {
        /// Environment id.
        id: String,
        /// Reachability subtitle.
        reachable: bool,
    },
    /// Quick model mention from the active harness's model catalog.
    Model {
        /// Wire model value.
        value: String,
        /// Display label.
        label: String,
    },
}

/// Source badge displayed on `$` skill rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOrigin {
    /// Repository skill.
    Repo,
    /// Project skill.
    Project,
    /// Personal / user skill.
    Personal,
    /// System skill.
    System,
    /// ACP provider / harness skill.
    Provider,
}

impl SkillOrigin {
    /// Short badge text rendered on the trailing edge of a `$` skill row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Repo => "Repo",
            Self::Project => "Project",
            Self::Personal => "Personal",
            Self::System => "System",
            Self::Provider => "Provider",
        }
    }
}

/// One selectable item in the tucked `$` skill drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// Skill name without `$`.
    pub name: String,
    /// Single-line description.
    pub description: String,
    /// Source badge label.
    pub origin: SkillOrigin,
    /// Ranking score (lower is better).
    pub score: u32,
}

/// One selectable item in the tucked `#` pull-request / issue drawer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubEntry {
    /// Underlying GitHub item.
    pub item: GitHubItem,
    /// Whether this PR is linked to the active thread's branch.
    pub linked: bool,
}

/// Unified accepted completion action returned when pressing Enter/Tab or
/// clicking a completion row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionChoice {
    /// Execute a built-in slash command immediately.
    Builtin(BuiltinCommand),
    /// Insert plain `/command ` text.
    SlashText(String),
    /// Insert an atomic [`rune::Mark::Pill`] replacing the trigger range.
    Pill {
        /// Typed pill id (`file:...`, `skill:...`, `pr:...`, etc.).
        id: String,
        /// Visible pill text (e.g., `@main.rs`, `$review`, `#42`).
        label: String,
    },
}

/// Parses an active `@`, `/`, `$`, or `#` completion token at the composer
/// caret, ignoring positions inside existing atomic pills and numeric currency
/// values such as `$100`.
#[must_use]
pub fn completion_token_at(
    text: &str,
    cursor: &DraftCursor,
    pills: &[DraftPill],
) -> Option<CompletionToken> {
    if !cursor.focused || cursor.anchor != cursor.focus {
        return None;
    }
    if pills
        .iter()
        .any(|pill| cursor.focus > pill.range.start && cursor.focus <= pill.range.end)
    {
        return None;
    }
    let before = text.get(..cursor.focus)?;
    let word = before.rsplit(char::is_whitespace).next()?;
    let start = before.len() - word.len();
    if pills.iter().any(|pill| ranges_overlap(&pill.range, start)) {
        return None;
    }
    let after = text.get(cursor.focus..)?;
    let end = cursor.focus + after.find(char::is_whitespace).unwrap_or(after.len());
    let mut chars = word.chars();
    let trigger = chars.next()?;
    let query = chars.as_str();
    let kind = match trigger {
        '/' => CompletionKind::Slash,
        '@' if !word.starts_with("@\"") => CompletionKind::Mention,
        '$' if !query.starts_with(|c: char| c.is_ascii_digit()) => CompletionKind::Skill,
        '#' => CompletionKind::PullRequest,
        _ => return None,
    };
    Some(CompletionToken {
        kind,
        range: start..end,
        query: query.to_owned(),
    })
}

const fn ranges_overlap(pill: &Range<usize>, token_start: usize) -> bool {
    token_start >= pill.start && token_start < pill.end
}

/// Commands advertised by the active thread or draft harness.
#[must_use]
pub fn available_commands(model: &Model) -> &[SlashCommand] {
    if let Some(state) = model.thread_state() {
        return &state.commands;
    }
    model
        .selection
        .new_thread
        .as_ref()
        .and_then(|draft| draft.active_harness(model.harnesses()))
        .or_else(|| model.harnesses().iter().find(|h| h.default))
        .or_else(|| model.harnesses().first())
        .map_or(&[], |harness| harness.commands.as_slice())
}

/// Total selectable item count for the active completion token.
#[must_use]
pub fn completion_count(model: &Model, token: &CompletionToken) -> usize {
    match token.kind {
        CompletionKind::Slash => {
            matching_commands(model, &token.query, token.range.start == 0).len()
        }
        CompletionKind::Mention => matching_mentions(model, &token.query).len(),
        CompletionKind::Skill => matching_skills(model, &token.query).len(),
        CompletionKind::PullRequest => matching_github(model, &token.query).len(),
    }
}

/// Resolves row `index` for `token` into a [`CompletionChoice`].
#[must_use]
pub fn completion_choice_at(
    model: &Model,
    token: &CompletionToken,
    index: usize,
) -> Option<CompletionChoice> {
    match token.kind {
        CompletionKind::Slash => {
            let list = matching_commands(model, &token.query, token.range.start == 0);
            let entry = list.get(index)?;
            Some(entry.builtin.map_or_else(
                || CompletionChoice::SlashText(format!("/{}", entry.name)),
                CompletionChoice::Builtin,
            ))
        }
        CompletionKind::Mention => {
            let list = matching_mentions(model, &token.query);
            let entry = list.get(index)?;
            Some(mention_to_choice(entry))
        }
        CompletionKind::Skill => {
            let list = matching_skills(model, &token.query);
            let entry = list.get(index)?;
            Some(CompletionChoice::Pill {
                id: DraftPill::skill_id(&entry.name),
                label: format!("${}", entry.name),
            })
        }
        CompletionKind::PullRequest => {
            let list = matching_github(model, &token.query);
            let entry = list.get(index)?;
            Some(CompletionChoice::Pill {
                id: DraftPill::github_id(&entry.item),
                label: format!("#{}", entry.item.number),
            })
        }
    }
}

fn mention_to_choice(entry: &MentionEntry) -> CompletionChoice {
    match entry {
        MentionEntry::File(file) => {
            let short = file
                .path
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(&file.path);
            CompletionChoice::Pill {
                id: DraftPill::file_id(&file.path, file.kind == FileSearchKind::Directory),
                label: format!("@{short}"),
            }
        }
        MentionEntry::Thread { id, title, .. } => CompletionChoice::Pill {
            id: DraftPill::thread_id(*id, title),
            label: format!("@{title}"),
        },
        MentionEntry::Environment { id, .. } => CompletionChoice::Pill {
            id: DraftPill::env_id(id),
            label: format!("@{id}"),
        },
        MentionEntry::Model { value, label } => CompletionChoice::Pill {
            id: DraftPill::model_id(value),
            label: format!("@{label}"),
        },
    }
}
