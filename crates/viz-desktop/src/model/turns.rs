//! Pure turn grouping and changed-files aggregation for T3-style timeline
//! rendering.

use std::collections::BTreeMap;

use viz_client::protocol::CompactionStatus;

use super::{
    CompactionNotice, ErrorKind, Item, MediaItem, Model, PlanCard, TaskResult, ToolStatus,
    Transcript, TurnEnd, User, diff_line_stats, relative_path,
};

/// One projected conversation turn ready for the timeline renderer.
#[derive(Debug)]
pub struct TurnView<'a> {
    /// Stable element key for the turn container.
    pub key: String,
    /// User messages that initiated (or stacked into) this turn.
    pub users: Vec<&'a User>,
    /// True when the turn began via `Continued` rather than a user prompt.
    pub continued: bool,
    /// Optional task-result arrival that initiated this turn.
    pub task_result: Option<&'a TaskResult>,
    /// Intermediate work-log entries (reasoning, tools, subagent messages).
    pub work: Vec<(&'a Item, bool)>,
    /// Plans reported in this turn.
    pub plans: Vec<&'a PlanCard>,
    /// Context compaction notices in this turn.
    pub compactions: Vec<&'a CompactionNotice>,
    /// Non-text media blocks emitted in this turn.
    pub media: Vec<&'a MediaItem>,
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

    /// Whether a context compaction is currently in progress in this turn.
    #[must_use]
    pub fn compacting(&self) -> bool {
        self.compactions
            .iter()
            .any(|notice| notice.status == CompactionStatus::InProgress)
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

/// Projects a [`Transcript`] into an ordered sequence of [`TurnView`]s.
#[must_use]
pub fn project_turns<'a>(
    model: &Model,
    transcript: &'a Transcript,
    busy: bool,
) -> Vec<TurnView<'a>> {
    let mut turns: Vec<TurnView<'a>> = Vec::new();
    let stream = transcript
        .displayed_history()
        .iter()
        .map(|item| (item, false))
        .chain(transcript.acp_partial.iter().map(|item| (item, true)));

    for (item, streaming) in stream {
        match item {
            Item::User(user) => {
                let can_stack = turns.last().is_some_and(|turn| {
                    !turn.continued
                        && turn.task_result.is_none()
                        && turn.work.is_empty()
                        && turn.plans.is_empty()
                        && turn.prose.is_empty()
                        && turn.end.is_none()
                });
                if can_stack {
                    if let Some(turn) = turns.last_mut() {
                        turn.users.push(user);
                    }
                } else {
                    turns.push(new_turn(format!("turn-{}", user.key), Some(user)));
                }
            }
            Item::Continued { key } => {
                let mut turn = new_turn(format!("turn-{key}"), None);
                turn.continued = true;
                turns.push(turn);
            }
            Item::TaskResult(result) => {
                let mut turn = new_turn(format!("turn-{}", result.key), None);
                turn.task_result = Some(result);
                turns.push(turn);
            }
            other => {
                if turns.is_empty() {
                    turns.push(new_turn(format!("turn-{}", other.key()), None));
                }
                let turn = turns
                    .last_mut()
                    .expect("turns is guaranteed non-empty above");
                push_turn_item(turn, other, streaming);
            }
        }
    }

    for turn in &mut turns {
        let mut files: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for (item, _) in &turn.work {
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
        turn.changed_files = files
            .into_iter()
            .map(|(path, (added, removed))| ChangedFile {
                path,
                added,
                removed,
            })
            .collect();
        turn.interrupted = turn
            .end
            .and_then(|end| end.stop_reason.as_deref())
            .is_some_and(|r| r == "cancelled")
            || turn
                .statuses
                .iter()
                .any(|(_, reason, _)| *reason == "cancelled");
    }

    if let Some(last) = turns.last_mut() {
        let has_active_work = last.work.iter().any(|(item, streaming)| {
            *streaming
                || matches!(
                    item,
                    Item::Tool(tool) if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
                )
        }) || last.prose.iter().any(|(_, streaming)| *streaming);
        last.live = last.end.is_none() && (busy || has_active_work);
    }

    turns
}

fn new_turn(key: String, user: Option<&User>) -> TurnView<'_> {
    TurnView {
        key,
        users: user.into_iter().collect(),
        continued: false,
        task_result: None,
        work: Vec::new(),
        plans: Vec::new(),
        compactions: Vec::new(),
        media: Vec::new(),
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
            turn.plans.push(plan);
        }
        Item::Compaction(notice) => {
            let s = (notice.started_at > 0).then_some(notice.started_at);
            let e = (notice.ended_at > 0).then_some(notice.ended_at);
            record_span(turn, s, e);
            turn.compactions.push(notice);
        }
        Item::Media(media) => turn.media.push(media),
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
        Item::User(_) | Item::Continued { .. } | Item::TaskResult(_) => {}
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
