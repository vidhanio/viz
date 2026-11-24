//! Pure folding of committed server entries, live ACP turn deltas, and lazily
//! hydrated entries into [`Transcript`].

use viz_client::protocol::{
    Body, Entry, EntryData, EntryId, LiveTurn, RunEnd, Segment, ToolCall, TranscriptEntry,
};

use super::{
    tool_parse::parse_tool,
    transcript::{
        CompactionNotice, ErrorKind, Item, MediaItem, PlanCard, TaskResult, ToolStatus, Transcript,
        TurnEnd, User,
    },
};

impl Transcript {
    /// Matches the separately watched ACP live state to the active prompt in
    /// the transcript.
    pub fn sync_acp(&mut self, live: Option<&LiveTurn>) {
        self.acp_partial.clear();
        let Some(live) = live.filter(|live| {
            !self.ended_run && self.acp_turn.as_ref().is_none_or(|turn| turn == &live.turn)
        }) else {
            return;
        };
        for entry in &live.entries {
            self.push_live_entry(&live.turn, entry);
        }
    }

    fn push_live_entry(&mut self, turn: &str, entry: &TranscriptEntry) {
        match &entry.body {
            Body::Text(segment) | Body::Thought(segment)
                if self.acp_text_index.is_none_or(|committed| {
                    text_index(&entry.key).is_some_and(|index| index > committed)
                }) =>
            {
                let thinking = matches!(&entry.body, Body::Thought(_));
                let settled = self
                    .items
                    .last()
                    .filter(|_| self.items.len() > self.acp_start);
                let preceding = match (thinking, settled) {
                    (
                        false,
                        Some(Item::Text {
                            key,
                            text,
                            started_at,
                            ..
                        }),
                    )
                    | (
                        true,
                        Some(Item::Thinking {
                            key,
                            text,
                            started_at,
                            ..
                        }),
                    ) => Some((key.as_str(), text.as_str(), *started_at)),
                    _ => None,
                };
                let (key, text, started_at) = preceding.map_or_else(
                    || (entry.key.clone(), segment.text.clone(), segment.started_at),
                    |(key, earlier, s)| {
                        (
                            key.to_owned(),
                            format!("{earlier}{}", segment.text),
                            s.or(segment.started_at),
                        )
                    },
                );
                let merged = Segment {
                    text,
                    message: segment.message.clone(),
                    started_at,
                    ended_at: segment.ended_at,
                };
                self.acp_partial
                    .push(Item::from_segment(key, merged, thinking));
            }
            Body::Tool(record) => {
                let committed = self.items[self.acp_start..]
                    .iter()
                    .any(|item| matches!(item, Item::Tool(tool) if tool.call_id == record.id));
                if !committed {
                    let tool = parse_tool(Some(turn), record);
                    self.acp_partial.push(Item::Tool(Box::new(tool)));
                }
            }
            Body::Plan(plan) => self.acp_partial.push(Item::Plan(PlanCard::from_wire(
                entry.key.clone(),
                None,
                None,
                plan,
            ))),
            Body::Compaction(c) => {
                self.acp_partial
                    .push(Item::Compaction(CompactionNotice::from_wire(
                        entry.key.clone(),
                        None,
                        None,
                        c,
                    )));
            }
            Body::Media(m) => self.acp_partial.push(Item::Media(MediaItem {
                key: entry.key.clone(),
                block: m.block.clone(),
                blob: m.blob.clone(),
            })),
            Body::Text(_) | Body::Thought(_) => {}
        }
    }

    /// Committed items excluding a trailing text item already merged into
    /// [`Self::acp_partial`].
    #[must_use]
    pub fn displayed_history(&self) -> &[Item] {
        if self
            .items
            .last()
            .zip(self.acp_partial.first())
            .is_some_and(|(last, first)| last.key() == first.key())
        {
            &self.items[..self.items.len() - 1]
        } else {
            &self.items
        }
    }

    /// Builds a transcript projection from a sequence of entries and an
    /// optional cursor for older history.
    #[must_use]
    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = &'a Entry>,
        before: Option<EntryId>,
    ) -> Self {
        let mut transcript = Self {
            before,
            ..Self::default()
        };
        transcript.append(entries);
        transcript
    }

    /// Appends committed server entries to the transcript in order.
    pub fn append<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>) {
        for entry in entries {
            self.entry(entry);
        }
    }

    /// Hydrates a trimmed stored entry in place after
    /// `ContentRequest::GetEntry` resolves its complete payload over the
    /// content plane.
    pub fn apply_loaded_entry(&mut self, id: EntryId, data: EntryData) {
        match data {
            EntryData::Transcript(segment) => {
                match segment.body {
                    Body::Tool(record) => {
                        if let Some(Item::Tool(tool)) = self
                            .items
                            .iter_mut()
                            .find(|item| matches!(item, Item::Tool(t) if t.entry == Some(id)))
                        {
                            let mut parsed = parse_tool(None, &record);
                            parsed.key.clone_from(&tool.key);
                            parsed.entry = Some(id);
                            parsed.full = None;
                            **tool = parsed;
                        }
                    }
                    Body::Plan(plan) => {
                        if let Some(Item::Plan(existing)) = self
                            .items
                            .iter_mut()
                            .find(|item| matches!(item, Item::Plan(p) if p.entry_id == Some(id)))
                        {
                            let key = existing.key.clone();
                            *existing = PlanCard::from_wire(key, Some(id), None, &plan);
                        }
                    }
                    Body::Compaction(c) => {
                        if let Some(Item::Compaction(existing)) = self.items.iter_mut().find(
                            |item| matches!(item, Item::Compaction(n) if n.entry_id == Some(id)),
                        ) {
                            let key = existing.key.clone();
                            *existing = CompactionNotice::from_wire(key, Some(id), None, &c);
                        }
                    }
                    Body::Text(_) | Body::Thought(_) | Body::Media(_) => {}
                }
            }
            EntryData::Prompt {
                text,
                attachments,
                context,
                origin,
            } => {
                if let Some(Item::User(user)) = self
                    .items
                    .iter_mut()
                    .find(|item| matches!(item, Item::User(u) if u.entry_id == Some(id)))
                {
                    user.text = text;
                    user.attachments = attachments;
                    user.context = context;
                    user.origin = origin;
                    user.full = None;
                }
            }
            EntryData::TaskResult { task, text } => {
                if let Some(Item::TaskResult(existing)) = self
                    .items
                    .iter_mut()
                    .find(|item| matches!(item, Item::TaskResult(r) if r.entry_id == Some(id)))
                {
                    let key = existing.key.clone();
                    *existing = TaskResult::parse(key, Some(id), None, &task, &text);
                }
            }
            EntryData::ToolUpdate { entry, record } => self.update_tool(entry, &record, None),
            EntryData::Run(_) | EntryData::Continued => {}
        }
    }

    fn entry(&mut self, Entry { id, full, data }: &Entry) {
        match data {
            EntryData::Prompt {
                text,
                attachments,
                context,
                origin,
            } => {
                let steered = self.acp_turn.is_some() && !self.ended_run;
                if !text.trim().is_empty() || !attachments.is_empty() || !context.is_empty() {
                    self.message_rows.insert(*id, format!("e{id}-0-row"));
                    self.items.push(Item::User(User {
                        key: format!("e{id}-0"),
                        entry_id: Some(*id),
                        full: *full,
                        text: text.clone(),
                        attachments: attachments.clone(),
                        context: context.clone(),
                        origin: origin.clone(),
                        steered,
                    }));
                }
                self.start(*id);
            }
            EntryData::Continued => {
                self.items.push(Item::Continued {
                    key: format!("e{id}"),
                });
                self.start(*id);
            }
            EntryData::TaskResult { task, text } => {
                self.message_rows.insert(*id, format!("e{id}"));
                self.items.push(Item::TaskResult(TaskResult::parse(
                    format!("e{id}"),
                    Some(*id),
                    *full,
                    task,
                    text,
                )));
                self.start(*id);
            }
            EntryData::ToolUpdate { entry, record } => self.update_tool(*entry, record, *full),
            EntryData::Transcript(entry) => self.transcript_entry(*id, *full, entry),
            EntryData::Run(end) => self.finish_run(*id, end),
        }
    }

    fn transcript_entry(&mut self, id: EntryId, full: Option<u64>, entry: &TranscriptEntry) {
        match &entry.body {
            Body::Text(segment) => {
                self.commit_text(&entry.key, segment, false);
                if let Some(item) = self.items.last() {
                    self.message_rows.insert(id, item.key().to_owned());
                }
            }
            Body::Thought(segment) => self.commit_text(&entry.key, segment, true),
            Body::Tool(record) => {
                let mut tool = parse_tool(self.acp_turn.as_deref(), record);
                tool.entry = Some(id);
                tool.full = full;
                self.items.push(Item::Tool(Box::new(tool)));
            }
            Body::Plan(plan) => {
                let projected = PlanCard::from_wire(format!("e{id}"), Some(id), full, plan);
                if let Some((prev_id, index)) = &self.plan
                    && *prev_id == plan.id
                    && let Some(Item::Plan(existing)) = self.items.get_mut(*index)
                {
                    existing.entry_id = Some(id);
                    existing.full = full;
                    existing.at = projected.at.or(existing.at);
                    existing.content = projected.content;
                } else {
                    self.plan = Some((plan.id.clone(), self.items.len()));
                    self.items.push(Item::Plan(projected));
                }
            }
            Body::Compaction(c) => {
                let projected = CompactionNotice::from_wire(format!("e{id}"), Some(id), full, c);
                if let Some(Item::Compaction(existing)) = self.items[self.acp_start..]
                    .iter_mut()
                    .find(|item| matches!(item, Item::Compaction(prev) if prev.id == c.id))
                {
                    existing.entry_id = Some(id);
                    existing.full = full;
                    existing.status = projected.status;
                    existing.summary = projected.summary;
                    existing.error = projected.error;
                    existing.tokens_before = projected.tokens_before.or(existing.tokens_before);
                    existing.tokens_after = projected.tokens_after.or(existing.tokens_after);
                    existing.ended_at = projected.ended_at;
                } else {
                    self.items.push(Item::Compaction(projected));
                }
            }
            Body::Media(m) => self.items.push(Item::Media(MediaItem {
                key: format!("e{id}"),
                block: m.block.clone(),
                blob: m.blob.clone(),
            })),
        }
    }

    fn finish_run(&mut self, id: EntryId, end: &RunEnd) {
        let mut prose_parts = Vec::new();
        let mut started_at: Option<i64> = None;
        let mut ended_at: Option<i64> = None;
        for item in &mut self.items[self.acp_start..] {
            match item {
                Item::Tool(tool) => {
                    if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) {
                        tool.status = ToolStatus::Abandoned;
                    }
                    started_at = min_opt(started_at, tool.started_at);
                    ended_at = max_opt(ended_at, tool.ended_at.or(tool.started_at));
                }
                Item::Text {
                    text,
                    started_at: s,
                    ended_at: e,
                    ..
                } => {
                    started_at = min_opt(started_at, *s);
                    ended_at = max_opt(ended_at, (*e).or(*s));
                    if !text.trim().is_empty() {
                        prose_parts.push(text.clone());
                    }
                }
                Item::Thinking {
                    started_at: s,
                    ended_at: e,
                    ..
                } => {
                    started_at = min_opt(started_at, *s);
                    ended_at = max_opt(ended_at, (*e).or(*s));
                }
                Item::Compaction(c) => {
                    started_at = min_opt(started_at, (c.started_at > 0).then_some(c.started_at));
                    ended_at = max_opt(ended_at, (c.ended_at > 0).then_some(c.ended_at));
                }
                _ => {}
            }
        }
        self.acp_turn = None;
        self.ended_run = true;
        self.plan = None;
        if let Some(text) = &end.error {
            self.items.push(Item::Notice {
                key: format!("e{id}"),
                kind: ErrorKind::classify(text),
                text: text.clone(),
            });
        } else if let Some(reason) = end
            .stop_reason
            .as_deref()
            .filter(|reason| !matches!(*reason, "end_turn" | "steered"))
        {
            let text = match reason {
                "cancelled" => "Run interrupted".to_owned(),
                "max_tokens" => "Reached the token limit".to_owned(),
                "max_turn_requests" => "Reached the request limit for this run".to_owned(),
                "refusal" => "The agent declined to continue".to_owned(),
                _ => format!("Run ended: {}", reason.replace('_', " ")),
            };
            self.items.push(Item::TurnStatus {
                key: format!("e{id}"),
                stop_reason: reason.to_owned(),
                text,
            });
        }
        let answer = if prose_parts.is_empty() {
            end.answer.clone().unwrap_or_default()
        } else {
            prose_parts.join("\n\n")
        };
        if !answer.is_empty() || end.usage.is_some() || end.stop_reason.is_some() {
            self.items.push(Item::TurnEnd(TurnEnd {
                key: format!("e{id}-end"),
                entry_id: Some(id),
                answer,
                stop_reason: end.stop_reason.clone(),
                usage: end.usage.clone(),
                started_at,
                ended_at,
            }));
        }
    }

    fn update_tool(&mut self, entry: EntryId, record: &ToolCall, full: Option<u64>) {
        if let Some(tool) = self.items.iter_mut().find_map(|item| match item {
            Item::Tool(tool) if tool.entry == Some(entry) => Some(tool),
            _ => None,
        }) {
            let mut updated = parse_tool(None, record);
            updated.key.clone_from(&tool.key);
            updated.entry = Some(entry);
            updated.full = full;
            **tool = updated;
        }
    }

    fn start(&mut self, id: EntryId) {
        self.acp_turn = Some(id.to_string());
        self.ended_run = false;
        self.acp_start = self.items.len();
        self.acp_text_index = None;
        self.plan = None;
    }

    fn commit_text(&mut self, key: &str, segment: &Segment, thinking: bool) {
        self.acp_text_index = text_index(key);
        if self.items.len() > self.acp_start {
            match (thinking, self.items.last_mut()) {
                (
                    true,
                    Some(Item::Thinking {
                        text: prev,
                        started_at: ps,
                        ended_at: pe,
                        ..
                    }),
                )
                | (
                    false,
                    Some(Item::Text {
                        text: prev,
                        started_at: ps,
                        ended_at: pe,
                        ..
                    }),
                ) => {
                    prev.push_str(&segment.text);
                    *ps = min_opt(*ps, segment.started_at);
                    *pe = max_opt(*pe, segment.ended_at);
                    return;
                }
                _ => {}
            }
        }
        self.items.push(Item::from_segment(
            key.to_owned(),
            segment.clone(),
            thinking,
        ));
    }
}

fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

fn text_index(key: &str) -> Option<usize> {
    key.rsplit_once(':')?.1.parse().ok()
}
