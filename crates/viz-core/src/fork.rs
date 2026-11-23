//! Fork a thread: which of its history the new thread starts with, and how the
//! agent there learns of it.

use viz_protocol::{EntryId, ForkPoint, RunId};

/// A run's place in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn {
    pub run: RunId,
    /// Its prompt, or what stands in for one.
    pub start: EntryId,
    /// `None` while it runs.
    pub end: Option<EntryId>,
}

/// The history a fork shares with its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    /// The last shared entry; `None` shares nothing.
    pub through: Option<EntryId>,
    pub runs: Vec<RunId>,
    /// Everything there is.
    pub head: bool,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("wait for the run to end before forking from it")]
    Unfinished,
    #[error("entry {0} is not a prompt of the thread")]
    NotAPrompt(EntryId),
    #[error("entry {0} is not in the thread")]
    NoEntry(EntryId),
}

/// Cut a transcript, given its started runs and its last entry.
///
/// A fork never shares part of a run: forking from an entry shares the run it
/// is in, and forking from the head of a busy thread leaves its current run
/// out.
///
/// # Errors
/// The point must be in the thread, and not in a run still under way.
pub fn cut(turns: &[Turn], last: Option<EntryId>, point: ForkPoint) -> Result<Cut, Rejection> {
    let before = |entry: EntryId| entry.0.checked_sub(1).filter(|&id| id > 0).map(EntryId);
    let running = turns.iter().find(|turn| turn.end.is_none());
    let through = match point {
        ForkPoint::Head => running.map_or(last, |turn| before(turn.start)),
        ForkPoint::Through { entry } => {
            if last.is_none_or(|last| entry > last) {
                return Err(Rejection::NoEntry(entry));
            }
            match turns
                .iter()
                .find(|turn| turn.start <= entry && turn.end.is_none_or(|end| entry <= end))
            {
                Some(Turn { end: Some(end), .. }) => Some(*end),
                Some(Turn { end: None, .. }) => return Err(Rejection::Unfinished),
                None => Some(entry),
            }
        }
        ForkPoint::Before { entry } => {
            if !turns.iter().any(|turn| turn.start == entry) {
                return Err(Rejection::NotAPrompt(entry));
            }
            before(entry)
        }
    };
    let runs = turns
        .iter()
        .filter(|turn| turn.end.is_some() && turn.end <= through)
        .map(|turn| turn.run)
        .collect();
    Ok(Cut {
        through,
        runs,
        head: running.is_none() && through == last,
    })
}

/// How the fork's first prompt gets its context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// The harness forks its own session.
    Native,
    /// The prompt carries the shared turns.
    Handoff,
    /// There is none.
    Nothing,
}

/// Choose the transfer. A harness forks a session as it is now, so only the
/// head of a saved session in the same directory can be forked natively.
#[must_use]
pub const fn transfer(cut: &Cut, native: bool) -> Transfer {
    if cut.runs.is_empty() {
        Transfer::Nothing
    } else if native && cut.head {
        Transfer::Native
    } else {
        Transfer::Handoff
    }
}
