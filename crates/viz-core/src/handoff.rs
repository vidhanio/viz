//! Assemble the context prefixed to a new session's first prompt.

/// The size of a handoff. The newest turn is kept even when it alone is
/// larger, in part.
const HANDOFF_BYTES: usize = 24 * 1024;

/// Why a conversation continues in a new session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason<'a> {
    /// Its thread moved to another directory.
    Moved { from: &'a str, to: &'a str },
    /// It was forked from another thread.
    Forked,
}

/// Build the handoff using transcript turns supplied newest first.
#[must_use]
pub fn note<'a>(
    reason: Reason<'_>,
    newest_first: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let reason = match reason {
        Reason::Moved { from, to } => {
            format!(
                "The working directory moved from {from} to {to}. This conversation continues in a new session"
            )
        }
        Reason::Forked => "This conversation was forked from another thread into a new session, and continues from there".to_owned(),
    };
    let mut turns = Vec::new();
    let mut size = 0;
    for (prompt, answer) in newest_first {
        let mut turn = format!("User: {prompt}\n\nAssistant: {answer}\n\n");
        if turns.is_empty() && turn.len() > HANDOFF_BYTES {
            turn.truncate(turn.floor_char_boundary(HANDOFF_BYTES));
            turn.push_str("…\n\n");
        }
        size += turn.len();
        if size > HANDOFF_BYTES && !turns.is_empty() {
            break;
        }
        turns.push(turn);
    }
    turns.reverse();
    format!(
        "<context>\n{reason}; its earlier turns, oldest first:\n\n{}</context>\n\n",
        turns.concat()
    )
}
