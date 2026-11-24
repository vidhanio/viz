//! Pure turn grouping and changed-files aggregation for T3-style timeline
//! rendering.

use std::collections::BTreeMap;

use viz_client::protocol::CompactionStatus;

use super::{
    ErrorKind, Item, Model, ToolStatus, Transcript, TurnEnd, diff_line_stats, relative_path,
};

/// One projected conversation turn ready for the timeline renderer.
#[derive(Debug)]
pub struct TurnView<'a> {
    /// Stable element key for the turn container.
    pub key: String,
    /// True when the turn began via `Continued` rather than a user prompt.
    pub continued: bool,
    /// Intermediate work-log entries (reasoning, tools, subagent messages).
    pub work: Vec<(&'a Item, bool)>,
    /// Whether compaction is currently in progress in this turn.
    pub compaction: Option<CompactionStatus>,
    /// Assistant prose blocks (item, streaming flag).
    pub prose: Vec<(&'a Item, bool)>,
    /// Non-default stop-reason dividers inside this turn.
    pub statuses: Vec<(&'a str, &'a str, &'a str)>,
    /// Inline error banners produced by this turn.
    pub notices: Vec<(&'a str, ErrorKind, &'a str)>,
    /// Settled turn boundary carrying answer text, stop reason, and usage.
    pub end: Option<&'a TurnEnd>,
    /// Files modified by tool calls in this turn, aggregated with line stats.
    pub changed_files: Vec<ChangedFile>,
    /// Whether this turn is currently running live.
    pub live: bool,
    /// Whether this turn was interrupted/cancelled by the user.
    pub interrupted: bool,
    /// Earliest known timestamp in Unix ms for elapsed/duration labels.
    pub started_at: Option<i64>,
    /// Latest known timestamp in Unix ms for completed duration & clock labels.
    pub ended_at: Option<i64>,
}

impl TurnView<'_> {
    /// Stable key for this turn's collapsible work fold header.
    #[must_use]
    pub fn fold_key(&self) -> String {
        format!("{}-fold", self.key)
    }

    /// Completed turn duration in milliseconds when timestamps are available.
    #[must_use]
    pub const fn duration_ms(&self) -> Option<i64> {
        match (self.started_at, self.ended_at) {
            (Some(start), Some(end)) if end >= start => Some(end - start),
            _ => None,
        }
    }

    /// Live elapsed milliseconds against `now_ms`.
    #[must_use]
    pub fn elapsed_ms(&self, now_ms: i64) -> Option<i64> {
        self.started_at
            .map(|start| now_ms.saturating_sub(start).max(0))
    }

    /// Total additions and deletions across all changed files in this turn.
    #[must_use]
    pub fn total_diff_stats(&self) -> (u32, u32) {
        self.changed_files.iter().fold((0, 0), |(add, del), file| {
            (
                add.saturating_add(file.added),
                del.saturating_add(file.removed),
            )
        })
    }
}

/// One aggregated file change in a turn's `ChangedFilesTree` card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Path relative to the thread's working directory.
    pub path: String,
    /// Total lines added across edits to this file in the turn.
    pub added: u32,
    /// Total lines removed across edits to this file in the turn.
    pub removed: u32,
}

/// An index into the displayed committed and live item stream.
#[derive(Debug)]
pub struct TurnSpec {
    pub key: String,
    pub start: usize,
    pub end: usize,
}

pub fn turn_items(
    transcript: &Transcript,
    start: usize,
    end: usize,
) -> impl Iterator<Item = (&Item, bool)> {
    let history = transcript.displayed_history();
    history[start.min(history.len())..end.min(history.len())]
        .iter()
        .map(|item| (item, false))
        .chain(
            transcript.acp_partial
                [start.saturating_sub(history.len())..end.saturating_sub(history.len())]
                .iter()
                .map(|item| (item, true)),
        )
}

/// Indexes turn boundaries without inspecting the contents of tools or prose.
pub fn turn_specs(transcript: &Transcript) -> Vec<TurnSpec> {
    let count = transcript.displayed_history().len() + transcript.acp_partial.len();
    let mut specs: Vec<TurnSpec> = Vec::new();
    let mut stack_users = false;
    for (index, (item, _)) in turn_items(transcript, 0, count).enumerate() {
        let starts_turn = match item {
            Item::User(_) => !stack_users,
            Item::Continued { .. } | Item::TaskResult(_) => true,
            _ => false,
        };
        if starts_turn || specs.is_empty() {
            specs.push(TurnSpec {
                key: format!("turn-{}", item.key()),
                start: index,
                end: index,
            });
            stack_users = !matches!(item, Item::Continued { .. } | Item::TaskResult(_));
        }
        specs.last_mut().expect("turn exists").end = index + 1;
        match item {
            Item::Thinking { .. } | Item::Text { .. } | Item::Plan(_) | Item::TurnEnd(_) => {
                stack_users = false;
            }
            Item::Tool(tool) if tool.review.is_none() || tool.status == ToolStatus::Failed => {
                stack_users = false;
            }
            _ => {}
        }
    }
    specs
}

/// Projects a mounted turn, including its file statistics and live state.
pub fn project_turn<'a>(
    model: &Model,
    transcript: &'a Transcript,
    spec: &TurnSpec,
    last: bool,
    with_files: bool,
) -> TurnView<'a> {
    let mut turn: TurnView<'a> = new_turn(spec.key.clone());
    for (item, streaming) in turn_items(transcript, spec.start, spec.end) {
        match item {
            Item::Continued { .. } => turn.continued = true,
            other => push_turn_item(&mut turn, other, streaming),
        }
    }

    if with_files {
        turn.changed_files = changed_files(model, &turn.work);
    }
    turn.interrupted = turn
        .end
        .and_then(|end| end.stop_reason.as_deref())
        .is_some_and(|r| r == "cancelled")
        || turn
            .statuses
            .iter()
            .any(|(_, reason, _)| *reason == "cancelled");
    turn.live = last && !model.detached() && turn.end.is_none() && (model.busy()
        || turn.work.iter().any(|(item, streaming)| {
            *streaming
                || matches!(
                    item,
                    Item::Tool(tool) if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
                )
        }) || turn.prose.iter().any(|(_, streaming)| *streaming));
    turn
}

fn changed_files(model: &Model, work: &[(&Item, bool)]) -> Vec<ChangedFile> {
    let mut files: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for (item, _) in work {
        if let Item::Tool(tool) = item {
            for diff in &tool.diffs {
                let rel = relative_path(model, &diff.path).to_owned();
                let (add, del) = diff_line_stats(diff);
                let entry = files.entry(rel).or_insert((0, 0));
                entry.0 = entry.0.saturating_add(add);
                entry.1 = entry.1.saturating_add(del);
            }
        }
    }
    files
        .into_iter()
        .map(|(path, (added, removed))| ChangedFile {
            path,
            added,
            removed,
        })
        .collect()
}

const fn new_turn(key: String) -> TurnView<'static> {
    TurnView {
        key,
        continued: false,
        work: Vec::new(),
        compaction: None,
        prose: Vec::new(),
        statuses: Vec::new(),
        notices: Vec::new(),
        end: None,
        changed_files: Vec::new(),
        live: false,
        interrupted: false,
        started_at: None,
        ended_at: None,
    }
}

fn push_turn_item<'a>(turn: &mut TurnView<'a>, item: &'a Item, streaming: bool) {
    match item {
        Item::Text {
            started_at,
            ended_at,
            ..
        } => {
            record_span(turn, *started_at, *ended_at);
            turn.prose.push((item, streaming));
        }
        Item::Thinking {
            started_at,
            ended_at,
            ..
        } => {
            record_span(turn, *started_at, *ended_at);
            turn.work.push((item, streaming));
        }
        Item::Tool(tool) => {
            if tool.review.is_some() && tool.status != ToolStatus::Failed {
                return;
            }
            record_span(turn, tool.started_at, tool.ended_at);
            turn.work.push((item, streaming));
        }
        Item::Plan(plan) => {
            record_span(turn, plan.at, plan.at);
        }
        Item::Compaction(notice) => {
            let s = (notice.started_at > 0).then_some(notice.started_at);
            let e = (notice.ended_at > 0).then_some(notice.ended_at);
            record_span(turn, s, e);
            if turn.compaction != Some(CompactionStatus::InProgress) {
                turn.compaction = Some(notice.status);
            }
        }
        Item::TurnStatus {
            key,
            stop_reason,
            text,
        } => turn
            .statuses
            .push((key.as_str(), stop_reason.as_str(), text.as_str())),
        Item::Notice { key, kind, text } => turn.notices.push((key.as_str(), *kind, text.as_str())),
        Item::TurnEnd(end) => {
            record_span(turn, end.started_at, end.ended_at);
            turn.end = Some(end);
        }
        Item::User(_) | Item::Continued { .. } | Item::TaskResult(_) | Item::Media(_) => {}
    }
}

fn record_span(turn: &mut TurnView<'_>, start: Option<i64>, end: Option<i64>) {
    if let Some(s) = start.filter(|&v| v > 0) {
        turn.started_at = Some(turn.started_at.map_or(s, |prev| prev.min(s)));
    }
    if let Some(e) = end.or(start).filter(|&v| v > 0) {
        turn.ended_at = Some(turn.ended_at.map_or(e, |prev| prev.max(e)));
    }
}
