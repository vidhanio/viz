//! Folds one turn's observed ACP output into committed entries and a live tail.

use std::{collections::HashSet, sync::Arc};

use agent_client_protocol::schema::MaybeUndefined;

use crate::{
    Body, Compaction, CompactionStatus, Entry, LiveTurn, Media, ObservedUpdate, Plan, PlanContent,
    PlanEntry, PlanPriority, PlanStatus, Receipts, Segment, TextKind, ToolCall, ToolPatch,
    receipts::ReceiptBook,
    schema::{
        CompactionStatus as AcpCompaction, ContentBlock, PlanEntryPriority, PlanEntryStatus,
        PlanUpdateContent, SessionUpdate, ToolCallStatus as AcpStatus,
    },
    tool::{ToolFields, ToolReport, block},
};

/// Commit streamed text at a chunk boundary once this many bytes accumulate.
const TEXT_RUN: usize = 4096;

/// A committed transcript entry or a report for a call the caller already
/// holds.
#[derive(Debug)]
pub enum Output {
    Entry(Box<Entry>),
    ToolPatch(Box<ToolPatch>),
}

impl From<Entry> for Output {
    fn from(entry: Entry) -> Self {
        Self::Entry(Box::new(entry))
    }
}

/// Folds a turn in observation order, using only the time on each update.
#[derive(Debug)]
pub struct Transcript {
    turn: String,
    receipts: Arc<Receipts>,
    tools: Vec<Entry>,
    /// Compactions still under way.
    compactions: Vec<Entry>,
    /// Compactions committed this turn; harnesses repeat terminal reports.
    finished_compactions: HashSet<String>,
    /// Latest reported context window usage (`used` tokens) observed in this
    /// turn, used to populate compaction before/after token counts.
    latest_usage: Option<u64>,
    patched_tools: HashSet<String>,
    answer: String,
    prior_prose: String,
    next_order: u64,
    text: Option<Entry>,
    message: Option<(TextKind, String)>,
}

impl Transcript {
    /// Starts the fold with the caller's turn key and turn-scoped MCP receipts.
    pub fn new(turn: String, receipts: Arc<Receipts>) -> Self {
        Self {
            turn,
            receipts,
            tools: Vec::new(),
            compactions: Vec::new(),
            finished_compactions: HashSet::new(),
            latest_usage: None,
            patched_tools: HashSet::new(),
            answer: String::new(),
            prior_prose: String::new(),
            next_order: 0,
            text: None,
            message: None,
        }
    }

    /// The last assistant message after the last tool call.
    #[must_use]
    pub fn answer(&self) -> &str {
        &self.answer
    }

    pub(crate) fn last_prose(&self) -> &str {
        if self.answer.is_empty() {
            &self.prior_prose
        } else {
            &self.answer
        }
    }

    /// Applies content updates and returns the entries and patches they commit.
    pub fn apply(&mut self, updates: Vec<ObservedUpdate>) -> Vec<Output> {
        let receipts = Arc::clone(&self.receipts);
        receipts.with_book(|book| self.fold(updates, book))
    }

    fn fold(&mut self, updates: Vec<ObservedUpdate>, receipts: &mut ReceiptBook) -> Vec<Output> {
        let mut output = Vec::new();
        for ObservedUpdate {
            update,
            observed_at,
        } in updates
        {
            match update {
                SessionUpdate::AgentMessageChunk(chunk) => {
                    self.chunk(TextKind::Text, chunk.content, observed_at, &mut output);
                }
                SessionUpdate::AgentThoughtChunk(chunk) => {
                    self.chunk(TextKind::Thought, chunk.content, observed_at, &mut output);
                }
                SessionUpdate::ToolCall(call) => {
                    let id = call.tool_call_id.0.to_string();
                    self.patched_tools.remove(&id);
                    self.tool_boundary(&mut output);
                    self.report_tool(id, call.into(), observed_at, receipts, &mut output);
                }
                SessionUpdate::ToolCallUpdate(update) => {
                    let id = update.tool_call_id.0.to_string();
                    let fields: ToolFields = update.into();
                    if self.tools.iter().any(|entry| tool(entry).id == id) {
                        self.report_tool(id, fields, observed_at, receipts, &mut output);
                    } else {
                        match fields.status() {
                            Some(AcpStatus::Pending | AcpStatus::InProgress) => {
                                if self.patched_tools.insert(id.clone()) {
                                    self.tool_boundary(&mut output);
                                }
                            }
                            Some(AcpStatus::Completed | AcpStatus::Failed) => {
                                self.patched_tools.remove(&id);
                            }
                            _ => {}
                        }
                        let order = self.next_order;
                        self.next_order += 1;
                        output.push(Output::ToolPatch(Box::new(ToolPatch {
                            id,
                            key: format!("{}:{order}", self.turn),
                            order,
                            fields,
                            observed_at,
                            receipts: Arc::clone(&self.receipts),
                        })));
                    }
                }
                SessionUpdate::Plan(plan) => {
                    self.plan(
                        Plan {
                            id: None,
                            at: Some(observed_at),
                            content: plan_items(plan.entries),
                        },
                        &mut output,
                    );
                }
                SessionUpdate::PlanUpdate(update) => {
                    let (id, content) = match update.plan {
                        PlanUpdateContent::Items(plan) => (plan.plan_id, plan_items(plan.entries)),
                        PlanUpdateContent::Markdown(plan) => (
                            plan.plan_id,
                            PlanContent::Markdown {
                                text: plan.content,
                                path: plan.path,
                            },
                        ),
                        _ => continue,
                    };
                    self.plan(
                        Plan {
                            id: Some(id.0.to_string()),
                            at: Some(observed_at),
                            content,
                        },
                        &mut output,
                    );
                }
                SessionUpdate::UsageUpdate(usage) => {
                    self.latest_usage = Some(usage.used);
                }
                SessionUpdate::CompactionUpdate(update) => {
                    self.compaction_update(update, observed_at, &mut output);
                }
                SessionUpdate::CompactionSummaryChunk(chunk)
                    if !self
                        .finished_compactions
                        .contains(chunk.compaction_id.0.as_ref()) =>
                {
                    let compaction =
                        self.compaction(chunk.compaction_id.0.to_string(), observed_at);
                    compaction.summary.push_str(&text_of(vec![chunk.content]));
                }
                _ => {}
            }
        }
        output
    }

    fn compaction_update(
        &mut self,
        update: crate::schema::CompactionUpdate,
        observed_at: i64,
        output: &mut Vec<Output>,
    ) {
        let status = match update.status {
            AcpCompaction::InProgress => CompactionStatus::InProgress,
            AcpCompaction::Completed => CompactionStatus::Completed,
            AcpCompaction::Failed => CompactionStatus::Failed,
            AcpCompaction::Cancelled => CompactionStatus::Cancelled,
            _ => return,
        };
        let id = update.compaction_id.0.to_string();
        if self.finished_compactions.contains(&id) {
            return;
        }
        let meta = match update.meta {
            MaybeUndefined::Value(meta) => Some(meta),
            MaybeUndefined::Null | MaybeUndefined::Undefined => None,
        };
        let meta_before = meta.as_ref().and_then(|m| {
            meta_u64(
                m,
                &["tokensBefore", "tokens_before", "preTokens", "pre_tokens"],
            )
        });
        let meta_after = meta.as_ref().and_then(|m| {
            meta_u64(
                m,
                &["tokensAfter", "tokens_after", "postTokens", "post_tokens"],
            )
        });
        let latest_usage = self.latest_usage;
        let compaction = self.compaction(id.clone(), observed_at);
        compaction.status = status;
        if let Some(before) = meta_before {
            compaction.tokens_before = Some(before);
        }
        if let Some(after) = meta_after {
            compaction.tokens_after = Some(after);
        } else if status == CompactionStatus::Completed
            && compaction.tokens_after.is_none()
            && let (Some(before), Some(now)) = (compaction.tokens_before, latest_usage)
            && now < before
        {
            compaction.tokens_after = Some(now);
        }
        match update.summary {
            MaybeUndefined::Value(blocks) => compaction.summary = text_of(blocks),
            MaybeUndefined::Null => compaction.summary.clear(),
            MaybeUndefined::Undefined => {}
        }
        match update.error {
            MaybeUndefined::Value(error) => compaction.error = Some(error),
            MaybeUndefined::Null => compaction.error = None,
            MaybeUndefined::Undefined => {}
        }
        if status != CompactionStatus::InProgress
            && let Some(index) = self
                .compactions
                .iter()
                .position(|entry| matches!(&entry.body, Body::Compaction(it) if it.id == id))
        {
            self.commit_text(output);
            output.push(self.compactions.remove(index).into());
            self.finished_compactions.insert(id);
        }
    }

    /// The compaction under way with this id, started when first seen.
    fn compaction(&mut self, id: String, observed_at: i64) -> &mut Compaction {
        let latest_usage = self.latest_usage;
        let index = self
            .compactions
            .iter()
            .position(|entry| matches!(&entry.body, Body::Compaction(it) if it.id == id))
            .unwrap_or_else(|| {
                let entry = self.allocate(Body::Compaction(Compaction {
                    id,
                    status: CompactionStatus::InProgress,
                    summary: String::new(),
                    error: None,
                    tokens_before: latest_usage,
                    tokens_after: None,
                    started_at: observed_at,
                    ended_at: observed_at,
                }));
                self.compactions.push(entry);
                self.compactions.len() - 1
            });
        let Body::Compaction(compaction) = &mut self.compactions[index].body else {
            unreachable!("compaction entries hold compactions")
        };
        compaction.ended_at = observed_at;
        compaction
    }

    /// The uncommitted entries in start order.
    #[must_use]
    pub fn live(&self) -> LiveTurn {
        let mut entries: Vec<_> = self
            .tools
            .iter()
            .chain(&self.compactions)
            .chain(self.text.iter())
            .cloned()
            .collect();
        entries.sort_unstable_by_key(|entry| entry.order);
        LiveTurn {
            turn: self.turn.clone(),
            entries,
        }
    }

    /// Whether this turn is still waiting for any known call to settle.
    #[must_use]
    pub fn has_open_tools(&self) -> bool {
        !self.tools.is_empty() || !self.patched_tools.is_empty()
    }

    /// Commits current prose and unfinished tools as they stand.
    pub fn finish(&mut self) -> Vec<Output> {
        self.patched_tools.clear();
        let mut output = Vec::new();
        self.commit_text(&mut output);
        output.extend(self.tools.drain(..).map(Output::from));
        output.extend(self.compactions.drain(..).map(Output::from));
        output
    }

    fn allocate(&mut self, body: Body) -> Entry {
        let order = self.next_order;
        self.next_order += 1;
        Entry {
            key: format!("{}:{order}", self.turn),
            order,
            body,
        }
    }

    fn chunk(
        &mut self,
        kind: TextKind,
        content: ContentBlock,
        observed_at: i64,
        output: &mut Vec<Output>,
    ) {
        let same_message = self
            .message
            .as_ref()
            .is_some_and(|(active, _)| *active == kind);
        if !same_message {
            self.commit_text(output);
            self.message = None;
        }
        let ContentBlock::Text(content) = content else {
            self.commit_text(output);
            output.push(
                self.allocate(Body::Media(Box::new(Media {
                    block: block(content),
                    blob: None,
                    source: kind,
                    at: observed_at,
                })))
                .into(),
            );
            return;
        };
        if kind == TextKind::Text {
            if !same_message && !self.answer.is_empty() {
                self.prior_prose = std::mem::take(&mut self.answer);
            }
            self.answer.push_str(&content.text);
        }
        let mut remaining = content.text.as_str();
        while !remaining.is_empty() {
            if self.text.is_none() {
                let segment = Segment {
                    text: String::new(),
                    message: String::new(),
                    started_at: Some(observed_at),
                    ended_at: Some(observed_at),
                };
                let mut entry = self.allocate(match kind {
                    TextKind::Text => Body::Text(segment),
                    TextKind::Thought => Body::Thought(segment),
                });
                let (_, message) = self
                    .message
                    .get_or_insert_with(|| (kind, entry.key.clone()));
                if let Body::Text(segment) | Body::Thought(segment) = &mut entry.body {
                    segment.message.clone_from(message);
                }
                self.text = Some(entry);
            }
            if let Some(entry) = &mut self.text
                && let Body::Text(segment) | Body::Thought(segment) = &mut entry.body
            {
                let room = TEXT_RUN.saturating_sub(segment.text.len()).max(1);
                let split = if remaining.len() <= room {
                    remaining.len()
                } else {
                    let boundary = remaining.floor_char_boundary(room);
                    if boundary == 0 {
                        remaining
                            .chars()
                            .next()
                            .map_or(remaining.len(), char::len_utf8)
                    } else {
                        boundary
                    }
                };
                let (head, tail) = remaining.split_at(split);
                segment.text.push_str(head);
                segment.ended_at = Some(observed_at);
                remaining = tail;
                if segment.text.len() >= TEXT_RUN {
                    self.commit_text(output);
                }
            }
        }
    }

    fn tool_boundary(&mut self, output: &mut Vec<Output>) {
        self.commit_text(output);
        self.message = None;
        if !self.answer.is_empty() {
            self.prior_prose = std::mem::take(&mut self.answer);
        }
    }

    fn report_tool(
        &mut self,
        id: String,
        fields: ToolFields,
        observed_at: i64,
        receipts: &mut ReceiptBook,
        output: &mut Vec<Output>,
    ) {
        let index = self
            .tools
            .iter()
            .position(|entry| tool(entry).id == id)
            .unwrap_or_else(|| {
                let entry = self.allocate(Body::Tool(Box::new(ToolCall::new(id))));
                self.tools.push(entry);
                self.tools.len() - 1
            });
        let entry = &mut self.tools[index];
        let Body::Tool(call) = &mut entry.body else {
            unreachable!("tool entries contain calls")
        };
        call.report(fields, receipts, observed_at);
        if call.status.is_settled() {
            self.commit_text(output);
            output.push(self.tools.remove(index).into());
        }
    }

    fn plan(&mut self, plan: Plan, output: &mut Vec<Output>) {
        self.commit_text(output);
        output.push(self.allocate(Body::Plan(plan)).into());
    }

    fn commit_text(&mut self, output: &mut Vec<Output>) {
        output.extend(self.text.take().map(Output::from));
    }
}

fn tool(entry: &Entry) -> &ToolCall {
    match &entry.body {
        Body::Tool(call) => call,
        _ => unreachable!("tool entries contain calls"),
    }
}

fn text_of(blocks: Vec<ContentBlock>) -> String {
    blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn plan_items(entries: Vec<crate::schema::PlanEntry>) -> PlanContent {
    let entries = entries
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
    PlanContent::Items { entries }
}

fn meta_u64(meta: &crate::schema::Meta, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| meta.get(*key).and_then(serde_json::Value::as_u64))
}
