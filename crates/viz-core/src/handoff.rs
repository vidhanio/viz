//! Assemble the context prefixed to the first prompt after a checkout move.

/// The newest turn is always kept, even when it exceeds the usual byte budget.
const HANDOFF_BYTES: usize = 24 * 1024;

/// Build the handoff using transcript turns supplied newest first.
#[must_use]
pub fn note<'a>(
    cwd: &str,
    from: &str,
    newest_first: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let moved = format!("The working directory moved from {from} to {cwd}.");
    let mut turns = Vec::new();
    let mut size = 0;
    for (prompt, answer) in newest_first {
        let turn = format!("User: {prompt}\n\nAssistant: {answer}\n\n");
        size += turn.len();
        if size > HANDOFF_BYTES && !turns.is_empty() {
            break;
        }
        turns.push(turn);
    }
    turns.reverse();
    format!(
        "<context>\n{moved} This conversation continues in a new session; its earlier turns, oldest first:\n\n{}</context>\n\n",
        turns.concat()
    )
}
