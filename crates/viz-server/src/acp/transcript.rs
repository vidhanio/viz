//! ACP `session/update` content folded into viz transcript entries and a
//! running turn's live tail.

use std::sync::Arc;

use agent_client_protocol::v1::schema::{
    ContentBlock, Plan, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
};

use super::tool_record::{Receipts, ToolContent, ToolFields};
use crate::wire::{
    LiveText, LiveTextKind, LiveTurn, NewEntry, PlanEntry, PlanPriority, PlanStatus, ToolCallStatus,
};

/// A text run this long (in bytes) is committed, and the next chunk starts a
/// new run.
const TEXT_RUN: usize = 4096;

/// One turn's agent output. Finished text runs, finished tools, and plans
/// become entries in arrival order; the current text run and unfinished tools
/// stay live until a later update or [`Transcript::finish`] commits them. Text
/// runs are keyed `<turn>:<n>`, so a client can match a live run to the entry
/// that commits it.
pub(super) struct Transcript {
    turn: String,
    receipts: Arc<Receipts>,
    tools: Vec<ToolContent>,
    answer: String,
    runs: u32,
    text: Option<LiveText>,
}

impl Transcript {
    /// `turn` is the live turn's identity; `receipts` link tool calls to viz's
    /// MCP tools.
    pub(super) const fn new(turn: String, receipts: Arc<Receipts>) -> Self {
        Self {
            turn,
            receipts,
            tools: Vec::new(),
            answer: String::new(),
            runs: 0,
            text: None,
        }
    }

    /// The agent's message text so far.
    pub(super) fn answer(&self) -> &str {
        &self.answer
    }

    /// Applies content updates, returning the entries they commit;
    /// session-level updates are ignored.
    pub(super) fn apply(&mut self, updates: Vec<SessionUpdate>) -> Vec<NewEntry> {
        let mut entries = Vec::new();
        for update in updates {
            match update {
                SessionUpdate::AgentMessageChunk(chunk) => {
                    self.chunk(LiveTextKind::Text, chunk.content, &mut entries);
                }
                SessionUpdate::AgentThoughtChunk(chunk) => {
                    self.chunk(LiveTextKind::Thought, chunk.content, &mut entries);
                }
                SessionUpdate::ToolCall(call) => {
                    let id = call.tool_call_id.0.to_string();
                    self.tool(id, call.into(), &mut entries);
                }
                SessionUpdate::ToolCallUpdate(update) => {
                    let id = update.tool_call_id.0.to_string();
                    self.tool(id, update.fields.into(), &mut entries);
                }
                SessionUpdate::Plan(plan) => {
                    self.commit_text(&mut entries);
                    entries.push(plan_entry(plan));
                }
                _ => {}
            }
        }
        entries
    }

    /// The uncommitted tail: live text before unfinished tools, the order
    /// [`Transcript::finish`] commits them in.
    pub(super) fn live(&self) -> LiveTurn {
        LiveTurn {
            turn: self.turn.clone(),
            text: self.text.clone(),
            tools: self
                .tools
                .iter()
                .map(|tool| tool.record().clone())
                .collect(),
        }
    }

    /// Commits the live text and unfinished tools as they stand.
    pub(super) fn finish(&mut self) -> Vec<NewEntry> {
        let mut entries = Vec::new();
        self.commit_text(&mut entries);
        entries.extend(
            self.tools
                .drain(..)
                .map(|tool| NewEntry::Tool(tool.record().clone())),
        );
        entries
    }

    fn chunk(&mut self, kind: LiveTextKind, content: ContentBlock, entries: &mut Vec<NewEntry>) {
        let ContentBlock::Text(content) = content else {
            return;
        };
        if kind == LiveTextKind::Text {
            self.answer.push_str(&content.text);
        }
        match &mut self.text {
            Some(run) if run.kind == kind => run.text.push_str(&content.text),
            _ => {
                self.commit_text(entries);
                let key = format!("{}:{}", self.turn, self.runs);
                self.runs += 1;
                self.text = Some(LiveText {
                    key,
                    kind,
                    text: content.text,
                });
            }
        }
        if self
            .text
            .as_ref()
            .is_some_and(|run| run.text.len() >= TEXT_RUN)
        {
            self.commit_text(entries);
        }
    }

    fn tool(&mut self, id: String, fields: ToolFields, entries: &mut Vec<NewEntry>) {
        let index = if let Some(index) = self.tools.iter().position(|tool| tool.record().id == id) {
            index
        } else {
            self.tools.push(ToolContent::new(id));
            self.tools.len() - 1
        };
        let status = self.tools[index].update(fields, &self.receipts).status;
        if matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed) {
            self.commit_text(entries);
            let tool = self.tools.remove(index);
            entries.push(NewEntry::Tool(tool.record().clone()));
        }
    }

    fn commit_text(&mut self, entries: &mut Vec<NewEntry>) {
        let Some(run) = self.text.take() else { return };
        entries.push(match run.kind {
            LiveTextKind::Text => NewEntry::Text {
                text: run.text,
                key: run.key,
            },
            LiveTextKind::Thought => NewEntry::Thought {
                text: run.text,
                key: run.key,
            },
        });
    }
}

fn plan_entry(plan: Plan) -> NewEntry {
    let entries = plan
        .entries
        .into_iter()
        .map(|entry| PlanEntry {
            content: entry.content,
            priority: match entry.priority {
                PlanEntryPriority::High => PlanPriority::High,
                PlanEntryPriority::Low => PlanPriority::Low,
                _ => PlanPriority::Medium,
            },
            status: match entry.status {
                PlanEntryStatus::InProgress => PlanStatus::InProgress,
                PlanEntryStatus::Completed => PlanStatus::Completed,
                _ => PlanStatus::Pending,
            },
        })
        .collect();
    NewEntry::Plan { entries }
}
