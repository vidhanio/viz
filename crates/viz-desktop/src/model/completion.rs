//! Pure inline composer completion derivations (`/` commands and `@` file
//! mentions).

use viz_client::protocol::SlashCommand;

use super::{CompletionKind, CompletionToken, DraftCursor, Model};

/// Parses a leading `/command` or inline `@file` token at the composer caret.
#[must_use]
pub fn completion_token_at(text: &str, cursor: &DraftCursor) -> Option<CompletionToken> {
    if !cursor.focused || cursor.anchor != cursor.focus {
        return None;
    }
    let before = text.get(..cursor.focus)?;
    let word = before.rsplit(char::is_whitespace).next()?;
    let start = before.len() - word.len();
    let after = text.get(cursor.focus..)?;
    let end = cursor.focus + after.find(char::is_whitespace).unwrap_or(after.len());
    let (kind, query) = match word.as_bytes().first().copied() {
        Some(b'/') if start == 0 => (CompletionKind::Slash, &word[1..]),
        Some(b'@') if !word.starts_with("@\"") => (CompletionKind::File, &word[1..]),
        _ => return None,
    };
    Some(CompletionToken {
        kind,
        range: start..end,
        query: query.to_owned(),
    })
}

/// Slash commands advertised by the open thread matching `query`.
#[must_use]
pub fn matching_commands<'a>(model: &'a Model, query: &str) -> Vec<&'a SlashCommand> {
    model
        .thread_state()
        .into_iter()
        .flat_map(|state| &state.commands)
        .filter(|command| {
            command
                .name
                .get(..query.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(query))
        })
        .collect()
}

/// File completion results for the active search request.
#[must_use]
pub fn matching_files(model: &Model) -> &[String] {
    model
        .composer
        .completion
        .files
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map_or(&[], Vec::as_slice)
}

/// Formats the inserted text for a chosen slash command or file mention.
#[must_use]
pub fn format_completion_insertion(kind: CompletionKind, id: &str, trailing_text: &str) -> String {
    let mut insertion = match kind {
        CompletionKind::Slash => format!("/{id}"),
        CompletionKind::File if id.contains([' ', '\t', '\n', '"', '\\']) => {
            format!("@\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\""))
        }
        CompletionKind::File => format!("@{id}"),
    };
    if !trailing_text.starts_with(char::is_whitespace) {
        insertion.push(' ');
    }
    insertion
}
