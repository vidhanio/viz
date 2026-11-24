//! Pure folding of committed server entries, live ACP turn deltas, anchored
//! pagination splices, and lazily hydrated entries into [`Transcript`].

use std::collections::HashSet;

use viz_client::protocol::{
    Body, Entry, EntryData, EntryId, EntryPage, LiveDelta, LiveTurn, RunEnd, Segment, ToolCall,
    TranscriptEntry,
};

use super::{
    tool_parse::parse_tool,
    transcript::{
        CompactionNotice, ErrorKind, Item, LiveEntryMeta, MediaItem, PlanCard, TaskResult,
        ToolStatus, Transcript, TurnEnd, User,
    },
};

impl Transcript {
    /// Increments [`Self::rev`] after mutating committed or live state.
    pub(super) const fn bump_rev(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Applies an incremental [`LiveDelta`] directly to [`Self::acp_partial`],
    /// updating only the changed entries in place without re-parsing untouched
    /// tools or reallocating committed prose prefixes.
    pub fn apply_live_delta(&mut self, delta: LiveDelta) {
        let Some(turn) = delta.turn.filter(|turn| {
            !self.ended_run && self.acp_turn.as_ref().is_none_or(|active| active == turn)
        }) else {
            if !self.acp_partial.is_empty() || self.live_turn.is_some() {
                self.acp_partial.clear();
                self.live_meta.clear();
                self.live_turn = None;
                self.bump_rev();
            }
            return;
        };

        if self.live_turn.as_deref() != Some(turn.as_str()) {
            self.acp_partial.clear();
            self.live_meta.clear();
            self.live_turn = Some(turn.clone());
        }

        if !delta.remove.is_empty() {
            let removing: HashSet<&str> = delta.remove.iter().map(String::as_str).collect();
            self.retain_live(|meta, _| !removing.contains(meta.wire_key.as_str()));
        }

        for entry in &delta.upsert {
            self.upsert_live_entry(&turn, entry);
        }

        self.sort_live_entries();
        self.bump_rev();
    }

    /// Replaces the live uncommitted ACP tail from a full [`LiveTurn`]
    /// snapshot (used on initial watch snapshot and scene fixtures).
    pub fn sync_acp(&mut self, live: Option<&LiveTurn>) {
        self.acp_partial.clear();
        self.live_meta.clear();
        let Some(live) = live.filter(|live| {
            !self.ended_run && self.acp_turn.as_ref().is_none_or(|turn| turn == &live.turn)
        }) else {
            self.live_turn = None;
            self.bump_rev();
            return;
        };
        self.live_turn = Some(live.turn.clone());
        for entry in &live.entries {
            self.upsert_live_entry(&live.turn, entry);
        }
        self.sort_live_entries();
        self.bump_rev();
    }

    /// Prunes live entries that were superseded by newly committed entries in
    /// [`Self::items`] and updates any merged committed-text prefix.
    fn reconcile_live_after_commit(&mut self) {
        if self.acp_partial.is_empty() {
            return;
        }
        if self.ended_run
            || self
                .acp_turn
                .as_ref()
                .zip(self.live_turn.as_ref())
                .is_some_and(|(active, live)| active != live)
        {
            self.acp_partial.clear();
            self.live_meta.clear();
            self.live_turn = None;
            return;
        }
        let acp_start = self.acp_start;
        let committed_text = self.acp_text_index;
        let items = &self.items;
        let mut i = 0usize;
        while i < self.acp_partial.len() {
            let remove = match &self.acp_partial[i] {
                Item::Text { .. } | Item::Thinking { .. } => committed_text.is_some_and(|c| {
                    text_index(&self.live_meta[i].wire_key).is_none_or(|idx| idx <= c)
                }),
                Item::Tool(tool) => items[acp_start..]
                    .iter()
                    .any(|item| matches!(item, Item::Tool(t) if t.call_id == tool.call_id)),
                _ => false,
            };
            if remove {
                self.acp_partial.remove(i);
                self.live_meta.remove(i);
            } else {
                i += 1;
            }
        }
        self.refresh_leading_live_text_prefix();
    }

    /// If the first live item is prose/thinking continuing the last committed
    /// item of the same kind, re-sync its committed prefix in place so
    /// [`Self::displayed_history`] never duplicates or drops newly committed
    /// segments.
    fn refresh_leading_live_text_prefix(&mut self) {
        let Some((first_item, first_meta)) =
            self.acp_partial.first_mut().zip(self.live_meta.first_mut())
        else {
            return;
        };
        let thinking = match first_item {
            Item::Text { .. } => false,
            Item::Thinking { .. } => true,
            _ => return,
        };
        let Some((committed_key, committed_text, committed_started)) =
            preceding_committed_text(&self.items, self.acp_start, thinking)
        else {
            return;
        };
        let (Item::Text {
            key,
            text,
            started_at,
            ..
        }
        | Item::Thinking {
            key,
            text,
            started_at,
            ..
        }) = first_item
        else {
            return;
        };
        if key == committed_key && first_meta.committed_prefix_len == committed_text.len() {
            return;
        }
        let live_tail = text[first_meta.committed_prefix_len.min(text.len())..].to_owned();
        text.clear();
        text.push_str(committed_text);
        first_meta.committed_prefix_len = committed_text.len();
        text.push_str(&live_tail);
        committed_key.clone_into(key);
        *started_at = committed_started.or(*started_at);
    }

    /// Upserts a streaming prose or thinking segment, merging it onto the
    /// preceding committed segment of the same kind when it continues one.
    fn upsert_live_text(
        &mut self,
        entry: &TranscriptEntry,
        segment: &Segment,
        thinking: bool,
        existing_idx: Option<usize>,
    ) {
        if self
            .acp_text_index
            .is_some_and(|committed| text_index(&entry.key).is_none_or(|index| index <= committed))
        {
            if let Some(idx) = existing_idx {
                self.acp_partial.remove(idx);
                self.live_meta.remove(idx);
            }
            return;
        }
        if let Some(idx) = existing_idx {
            let prefix_len = self.live_meta[idx].committed_prefix_len;
            self.live_meta[idx].order = entry.order;
            match &mut self.acp_partial[idx] {
                Item::Text { text, ended_at, .. } | Item::Thinking { text, ended_at, .. } => {
                    text.truncate(prefix_len.min(text.len()));
                    text.push_str(&segment.text);
                    *ended_at = segment.ended_at;
                    return;
                }
                _ => {
                    self.acp_partial.remove(idx);
                    self.live_meta.remove(idx);
                }
            }
        }
        let preceding = preceding_committed_text(&self.items, self.acp_start, thinking);
        let (key, text, started_at, prefix_len) = preceding.map_or_else(
            || {
                (
                    entry.key.clone(),
                    segment.text.clone(),
                    segment.started_at,
                    0,
                )
            },
            |(key, earlier, s)| {
                let mut buf = String::with_capacity(earlier.len() + segment.text.len());
                buf.push_str(earlier);
                buf.push_str(&segment.text);
                (key.to_owned(), buf, s.or(segment.started_at), earlier.len())
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
        self.live_meta.push(LiveEntryMeta {
            wire_key: entry.key.clone(),
            order: entry.order,
            committed_prefix_len: prefix_len,
        });
    }

    fn upsert_live_entry(&mut self, turn: &str, entry: &TranscriptEntry) {
        let existing_idx = self
            .live_meta
            .iter()
            .position(|meta| meta.wire_key == entry.key);
        match &entry.body {
            Body::Text(segment) | Body::Thought(segment) => {
                let thinking = matches!(&entry.body, Body::Thought(_));
                self.upsert_live_text(entry, segment, thinking, existing_idx);
            }
            Body::Tool(record) => {
                let committed = self.items[self.acp_start..]
                    .iter()
                    .any(|item| matches!(item, Item::Tool(tool) if tool.call_id == record.id));
                if committed {
                    if let Some(idx) = existing_idx {
                        self.acp_partial.remove(idx);
                        self.live_meta.remove(idx);
                    }
                    return;
                }
                let parsed = Item::Tool(Box::new(parse_tool(Some(turn), record)));
                if let Some(idx) = existing_idx {
                    self.acp_partial[idx] = parsed;
                    self.live_meta[idx].order = entry.order;
                } else {
                    self.acp_partial.push(parsed);
                    self.live_meta.push(LiveEntryMeta {
                        wire_key: entry.key.clone(),
                        order: entry.order,
                        committed_prefix_len: 0,
                    });
                }
            }
            Body::Plan(plan) => {
                let item = Item::Plan(PlanCard::from_wire(entry.key.clone(), None, None, plan));
                self.replace_or_push_live(existing_idx, entry, item);
            }
            Body::Compaction(c) => {
                let item = Item::Compaction(CompactionNotice::from_wire(
                    entry.key.clone(),
                    None,
                    None,
                    c,
                ));
                self.replace_or_push_live(existing_idx, entry, item);
            }
            Body::Media(m) => {
                let item = Item::Media(MediaItem {
                    key: entry.key.clone(),
                    block: m.block.clone(),
                    blob: m.blob.clone(),
                });
                self.replace_or_push_live(existing_idx, entry, item);
            }
        }
    }

    fn replace_or_push_live(
        &mut self,
        existing_idx: Option<usize>,
        entry: &TranscriptEntry,
        item: Item,
    ) {
        if let Some(idx) = existing_idx {
            self.acp_partial[idx] = item;
            self.live_meta[idx].order = entry.order;
        } else {
            self.acp_partial.push(item);
            self.live_meta.push(LiveEntryMeta {
                wire_key: entry.key.clone(),
                order: entry.order,
                committed_prefix_len: 0,
            });
        }
    }

    fn retain_live(&mut self, mut keep: impl FnMut(&LiveEntryMeta, &Item) -> bool) {
        let mut i = 0usize;
        while i < self.acp_partial.len() {
            if keep(&self.live_meta[i], &self.acp_partial[i]) {
                i += 1;
            } else {
                self.acp_partial.remove(i);
                self.live_meta.remove(i);
            }
        }
    }

    fn sort_live_entries(&mut self) {
        if self.acp_partial.len() <= 1 {
            return;
        }
        let sorted = self.live_meta.windows(2).all(|w| w[0].order <= w[1].order);
        if sorted {
            return;
        }
        let mut paired: Vec<(LiveEntryMeta, Item)> = self
            .live_meta
            .drain(..)
            .zip(self.acp_partial.drain(..))
            .collect();
        paired.sort_by_key(|(meta, _)| meta.order);
        for (meta, item) in paired {
            self.live_meta.push(meta);
            self.acp_partial.push(item);
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
            rev: 1,
            before,
            ..Self::default()
        };
        transcript.append(entries);
        transcript
    }

    /// Appends committed server entries to the transcript in order, reconciling
    /// the live ACP tail without dropping unaffected live entries.
    pub fn append<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>) {
        let mut any = false;
        for entry in entries {
            any = true;
            self.entry(entry);
        }
        if any {
            self.reconcile_live_after_commit();
            self.bump_rev();
        }
    }

    /// Splices an older [`EntryPage`] (`EntryAnchor::Before`) in front of the
    /// loaded transcript in `O(|page|)` time without re-folding existing pages
    /// or discarding hydrated entries.
    ///
    /// When the existing window began mid-turn (only possible if a single turn
    /// exceeded page budget), only that leading incomplete turn's raw entries
    /// are re-folded together with `page`.
    pub fn prepend(&mut self, page: &EntryPage) {
        self.before = page.before;
        if page.entries.is_empty() {
            self.bump_rev();
            return;
        }

        let prefix = if self.leading_raw.is_empty() {
            Self::from_entries(page.entries.iter(), page.before)
        } else {
            let removed_items = self.leading_item_count.min(self.items.len());
            self.items.drain(..removed_items);
            self.acp_start = self.acp_start.saturating_sub(removed_items);
            if let Some((_, plan_idx)) = &mut self.plan {
                if *plan_idx < removed_items {
                    self.plan = None;
                } else {
                    *plan_idx -= removed_items;
                }
            }
            for raw in &self.leading_raw {
                self.message_rows.remove(&raw.id);
            }
            Self::from_entries(
                page.entries.iter().chain(self.leading_raw.iter()),
                page.before,
            )
        };

        let shift = prefix.items.len();
        self.acp_start = self.acp_start.saturating_add(shift);
        if let Some((_, plan_idx)) = &mut self.plan {
            *plan_idx = plan_idx.saturating_add(shift);
        }
        self.message_rows.extend(prefix.message_rows);
        self.leading_raw = prefix.leading_raw;
        self.leading_item_count = prefix.leading_item_count;
        self.starts_at_turn_boundary = prefix.starts_at_turn_boundary;
        self.has_entries = true;

        let mut combined = prefix.items;
        combined.append(&mut self.items);
        self.items = combined;
        self.bump_rev();
    }

    /// Hydrates a trimmed stored entry in place after
    /// `ContentRequest::GetEntry` resolves its complete payload over the
    /// content plane.
    pub fn apply_loaded_entry(&mut self, id: EntryId, data: EntryData) {
        let mut changed = false;
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
                            changed = true;
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
                            changed = true;
                        }
                    }
                    Body::Compaction(c) => {
                        if let Some(Item::Compaction(existing)) = self.items.iter_mut().find(
                            |item| matches!(item, Item::Compaction(n) if n.entry_id == Some(id)),
                        ) {
                            let key = existing.key.clone();
                            *existing = CompactionNotice::from_wire(key, Some(id), None, &c);
                            changed = true;
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
                    changed = true;
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
                    changed = true;
                }
            }
            EntryData::ToolUpdate { entry, record } => {
                self.update_tool(entry, &record, None);
                changed = true;
            }
            EntryData::Run(_) | EntryData::Continued => {}
        }
        if changed {
            self.bump_rev();
        }
    }

    fn entry(&mut self, entry: &Entry) {
        let is_turn_start = matches!(
            entry.data,
            EntryData::Prompt { .. } | EntryData::Continued | EntryData::TaskResult { .. }
        );
        if !self.has_entries {
            self.has_entries = true;
            self.starts_at_turn_boundary = is_turn_start;
        }
        let record_leading = !self.starts_at_turn_boundary && self.acp_turn.is_none();
        if record_leading && !is_turn_start {
            self.leading_raw.push(entry.clone());
        }

        let Entry { id, full, data } = entry;
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

        if record_leading && !is_turn_start {
            self.leading_item_count = self.items.len();
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

fn preceding_committed_text(
    items: &[Item],
    acp_start: usize,
    thinking: bool,
) -> Option<(&str, &str, Option<i64>)> {
    let settled = items.last().filter(|_| items.len() > acp_start)?;
    match (thinking, settled) {
        (
            false,
            Item::Text {
                key,
                text,
                started_at,
                ..
            },
        )
        | (
            true,
            Item::Thinking {
                key,
                text,
                started_at,
                ..
            },
        ) => Some((key.as_str(), text.as_str(), *started_at)),
        _ => None,
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
