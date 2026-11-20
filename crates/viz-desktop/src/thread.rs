//! The watched conversation as the window shows it, read from a pi-durable
//! `ConversationView` (`value.entries` and `value.docs`). Shapes are
//! pi-durable's `EntryRecord`, pi-ai messages and live document;
//! an unexpected entry is skipped rather than failing the whole view.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use viz_client::protocol::{
    ConversationId, EntryId, LiveTextKind, LiveTurn, ToolCallStatus, ToolDiff, ToolRecord,
};

/// Longest tool-call summary, in characters.
const SUMMARY_CHARS: usize = 96;
/// Longest tool argument listing an expanded row shows, in characters.
const ARGUMENTS_CHARS: usize = 4_000;
/// Longest tool output an expanded row shows, in characters (its tail for
/// running output, its head for a result).
const OUTPUT_CHARS: usize = 6_000;

/// The conversation's transcript window, live state and settings.
#[derive(Debug, Clone, Default)]
pub struct Thread {
    /// Timeline items in transcript order.
    pub items: Vec<Item>,
    /// Exclusive cursor for the next page of older entries.
    pub before: Option<EntryId>,
    /// What's happening right now (`pi.live`).
    pub live: Live,
    acp_turn: Option<String>,
    acp_start: usize,
    acp_text_index: Option<usize>,
    pub acp_partial: Vec<Item>,
}

/// One timeline row.
#[derive(Debug, Clone)]
pub enum Item {
    /// Something I wrote.
    User(User),
    /// Assistant prose, Markdown.
    Text {
        /// Stable element key.
        key: String,
        /// The Markdown source.
        text: String,
    },
    /// The assistant's reasoning, collapsed by default.
    Thinking {
        /// Stable element key.
        key: String,
        /// The reasoning text (empty when redacted).
        text: String,
        /// About how long it took, once its message is done: the message's
        /// time in proportion to the share of its output the reasoning is.
        duration_ms: Option<u64>,
    },
    /// A tool call and, once settled, its result.
    Tool(Box<Tool>),
    /// The latest checklist reported in this ACP turn.
    Plan { key: String, steps: Vec<PlanStep> },
    /// A harness progress message.
    Progress { key: String, text: String },
    /// A turn boundary (no label) or another marker that isn't a message.
    Divider {
        /// Stable element key.
        key: String,
        /// What it marks, when it's more than a new turn.
        label: Option<&'static str>,
    },
    /// An assistant reply that ended without an answer.
    Notice {
        /// Stable element key.
        key: String,
        /// Why.
        text: String,
    },
}

/// Something I wrote. Single-user: every input is mine, whichever client or
/// connector it came through.
#[derive(Debug, Clone)]
pub struct User {
    /// Stable element key.
    pub key: String,
    /// What I wrote.
    pub text: String,
    /// Names of the stored attachments carried by this prompt.
    pub attachments: Vec<String>,
}

impl Item {
    pub fn key(&self) -> &str {
        match self {
            Self::User(user) => &user.key,
            Self::Tool(tool) => &tool.key,
            Self::Text { key, .. }
            | Self::Thinking { key, .. }
            | Self::Plan { key, .. }
            | Self::Progress { key, .. }
            | Self::Divider { key, .. }
            | Self::Notice { key, .. } => key,
        }
    }
}
/// A tool call's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Queued behind other calls of its round, or still being written.
    Pending,
    /// Executing.
    Running,
    /// Finished with a result.
    Done,
    /// Finished with an error.
    Failed,
    /// No result and nothing running it: the run ended first.
    Abandoned,
}

/// A tool call row.
#[derive(Debug, Clone)]
pub struct Tool {
    /// Stable element key.
    pub key: String,
    /// The call's id, pairing it with its result.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// One-line summary of the arguments.
    pub summary: String,
    /// Recognized human-readable details, never a serialized arguments object.
    pub input: Option<String>,
    pub timeout_ms: Option<u64>,
    pub started_at: Option<i64>,
    /// Progress.
    pub status: ToolStatus,
    /// The result text, or running output, capped.
    pub output: String,
    /// How long it ran, once done.
    pub duration_ms: Option<u64>,
    /// The conversation a subagent call delegated to.
    pub child: Option<ConversationId>,
    /// The viz tool an ACP call ran (`spawn_subagent`, `read_subagent`, …).
    pub viz: Option<String>,
    pub diffs: Vec<ToolDiff>,
    pub locations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub text: String,
    pub status: ToolStatus,
}

/// `pi.live`, as the window shows it.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// A native run or ACP turn is in progress.
    pub busy: bool,
    /// The in-flight answer (`generation.message`), parsed like a committed
    /// one.
    pub partial: Vec<Item>,
    /// The generation is waiting to retry after this error.
    pub retry: Option<String>,
    /// Why the newest run ended without an answer.
    pub failure: Option<String>,
    /// A compaction is running.
    pub compacting: bool,
}

impl Thread {
    /// Match the separately watched ACP state to the prompt in the transcript.
    pub fn sync_acp(&mut self, live: Option<&LiveTurn>) {
        let Some(live) = live.filter(|live| self.acp_turn.as_ref() == Some(&live.turn)) else {
            return;
        };
        self.acp_partial.clear();
        if let Some(text) = &live.text
            && self.acp_text_index.is_none_or(|committed| {
                text_index(&text.key).is_some_and(|index| index > committed)
            })
        {
            let settled = self
                .items
                .last()
                .filter(|_| self.items.len() > self.acp_start);
            let preceding = match (text.kind, settled) {
                (LiveTextKind::Text, Some(Item::Text { key, text }))
                | (LiveTextKind::Thought, Some(Item::Thinking { key, text, .. })) => {
                    Some((key, text))
                }
                _ => None,
            };
            let thinking = text.kind == LiveTextKind::Thought;
            let (key, text) = preceding.map_or_else(
                || (text.key.clone(), text.text.clone()),
                |(key, earlier)| (key.clone(), format!("{earlier}{}", text.text)),
            );
            self.acp_partial.push(acp_text(key, text, thinking));
        }
        for record in &live.tools {
            // The view and state arrive separately; never reintroduce a
            // live row after the transcript has committed the same call.
            let committed = self.items[self.acp_start..]
                .iter()
                .any(|item| matches!(item, Item::Tool(tool) if tool.call_id == record.id));
            if !committed {
                self.acp_partial.push(Item::Tool(Box::new(Tool::from_acp(
                    Some(&live.turn),
                    record.clone(),
                ))));
            }
        }
    }

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

    /// Reads a conversation view's value.
    pub fn from_view(view: &Value) -> Self {
        Self::from_history(view, &[], before_cursor(view))
    }

    pub fn from_history(view: &Value, earlier: &[Value], before: Option<EntryId>) -> Self {
        let live = view.pointer("/docs/pi.live");
        let slots = live
            .and_then(|live| live.get("tools"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mut reader = Reader {
            items: Vec::new(),
            calls: HashMap::new(),
            plan: None,
            acp_turn: None,
            acp_start: 0,
            acp_text_index: None,
        };
        for entry in earlier.iter().chain(array(view.get("entries"))) {
            reader.entry(entry);
        }
        reader.live_tools(slots);
        let native_busy = live.is_some_and(|live| live.get("run").is_some());
        let acp_busy = reader.acp_turn.is_some();
        let mut items = reader.items;
        for item in &mut items {
            if let Item::Tool(tool) = item
                && !native_busy
                && !acp_busy
                && matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
            {
                tool.status = ToolStatus::Abandoned;
            }
        }
        let mut live = live.map_or_default(|live| Live::read(live, native_busy));
        live.busy |= acp_busy;
        Self {
            items,
            before,
            live,
            acp_turn: reader.acp_turn,
            acp_start: reader.acp_start,
            acp_text_index: reader.acp_text_index,
            acp_partial: Vec::new(),
        }
    }
}

pub fn before_cursor(view: &Value) -> Option<EntryId> {
    // pi-durable's cursor is the last omitted entry, while conversation.entries
    // uses an exclusive upper bound.
    view.get("before")?.as_u64()?.checked_add(1).map(EntryId)
}

/// Builds items from entries in order, pairing tool results with their calls.
struct Reader {
    items: Vec<Item>,
    /// Item index of each tool call by call id.
    calls: HashMap<String, usize>,
    plan: Option<usize>,
    acp_turn: Option<String>,
    acp_start: usize,
    acp_text_index: Option<usize>,
}

impl Reader {
    fn entry(&mut self, entry: &Value) {
        let id = entry
            .get("id")
            .map_or_else(String::new, ToString::to_string);
        let kind = str_at(entry, "kind").unwrap_or_default();
        let messages = array(entry.get("model"));
        match kind {
            "pi.reset" => self.items.push(Item::Divider {
                key: format!("e{id}"),
                label: None,
            }),
            "pi.compaction" => self.items.push(Item::Divider {
                key: format!("e{id}"),
                label: Some("Earlier context summarized"),
            }),
            "viz.agent.prompt" => {
                self.acp_turn = Some(id.clone());
                self.acp_text_index = None;
                self.prompt(&id, entry, messages);
                self.acp_start = self.items.len();
            }
            "pi.user" | "viz.input" => {
                self.prompt(&id, entry, messages);
            }
            "pi.assistant" => {
                for (m, message) in messages.iter().enumerate() {
                    let key = format!("e{id}-{m}");
                    let at = self.items.len();
                    assistant(&key, message, &mut self.items, ToolStatus::Pending);
                    for (i, item) in self.items.iter().enumerate().skip(at) {
                        if let Item::Tool(tool) = item {
                            self.calls.insert(tool.call_id.clone(), i);
                        }
                    }
                }
            }
            "pi.tool-result" => {
                for message in messages {
                    self.result(message);
                }
            }
            kind if kind.starts_with("viz.agent.") => self.acp_entry(kind, &id, entry),
            _ => {}
        }
    }

    fn prompt(&mut self, id: &str, entry: &Value, messages: &[Value]) {
        let attachments: Vec<_> = array(entry.pointer("/data/attachments"))
            .iter()
            .filter_map(|file| str_at(file, "name"))
            .map(str::to_owned)
            .collect();
        // A connector's input reaches the model with a routing prefix; show
        // the unmodified text and only the attachment names.
        if let Some(text) = entry.pointer("/data/text").and_then(Value::as_str) {
            if !text.trim().is_empty() || !attachments.is_empty() {
                self.items.push(Item::User(User {
                    key: format!("e{id}-0"),
                    text: text.to_owned(),
                    attachments,
                }));
            }
            return;
        }
        for (index, message) in messages.iter().enumerate() {
            let text = content_text(message.get("content"));
            if !text.trim().is_empty() {
                self.items.push(Item::User(User {
                    key: format!("e{id}-{index}"),
                    text,
                    attachments: Vec::new(),
                }));
            }
        }
    }

    /// Reads display-only ACP updates without treating them as native model
    /// messages.
    fn acp_entry(&mut self, kind: &str, id: &str, entry: &Value) {
        match kind {
            "viz.agent.text" | "viz.agent.thought" => {
                let Some(text) = entry.pointer("/data/text").and_then(Value::as_str) else {
                    return;
                };
                let key = entry.pointer("/data/key").and_then(Value::as_str);
                if let Some(index) = key.and_then(text_index) {
                    self.acp_text_index = Some(index);
                }
                match (kind, self.items.last_mut()) {
                    ("viz.agent.text", Some(Item::Text { text: previous, .. }))
                    | ("viz.agent.thought", Some(Item::Thinking { text: previous, .. })) => {
                        previous.push_str(text);
                    }
                    _ => self.items.push(acp_text(
                        key.map_or_else(|| format!("e{id}"), str::to_owned),
                        text.to_owned(),
                        kind == "viz.agent.thought",
                    )),
                }
            }
            "viz.agent.tool" => {
                if let Some(data) = entry.get("data")
                    && let Ok(record) = ToolRecord::deserialize(data)
                {
                    self.items.push(Item::Tool(Box::new(Tool::from_acp(
                        self.acp_turn.as_deref(),
                        record,
                    ))));
                }
            }
            "viz.agent.plan" => self.plan(id, array(entry.pointer("/data/entries"))),
            "viz.agent.progress" => {
                if let Some(text) = entry.pointer("/data/text").and_then(Value::as_str) {
                    self.items.push(Item::Progress {
                        key: format!("e{id}"),
                        text: text.to_owned(),
                    });
                }
            }
            "viz.agent.turn" => {
                for item in &mut self.items[self.acp_start..] {
                    if let Item::Tool(tool) = item
                        && matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
                    {
                        tool.status = ToolStatus::Abandoned;
                    }
                }
                self.acp_turn = None;
                self.plan = None;
                if let Some(data) = entry.get("data") {
                    let key = format!("e{id}");
                    if let Some(error) = str_at(data, "error") {
                        self.items.push(Item::Notice {
                            key,
                            text: error.to_owned(),
                        });
                    } else if let Some(reason) =
                        str_at(data, "stopReason").filter(|reason| *reason != "end_turn")
                    {
                        let text = match reason {
                            "cancelled" => "Stopped".to_owned(),
                            "max_tokens" => "Reached the token limit".to_owned(),
                            "max_turn_requests" => {
                                "Reached the request limit for this turn".to_owned()
                            }
                            "refusal" => "The agent declined to continue".to_owned(),
                            _ => format!("Turn ended: {}", reason.replace('_', " ")),
                        };
                        self.items.push(Item::Progress { key, text });
                    }
                }
            }
            _ => {}
        }
    }

    fn plan(&mut self, id: &str, entries: &[Value]) {
        let steps = entries
            .iter()
            .filter_map(|entry| {
                Some(PlanStep {
                    text: str_at(entry, "content")?.to_owned(),
                    status: match str_at(entry, "status") {
                        Some("completed") => ToolStatus::Done,
                        Some("in_progress") => ToolStatus::Running,
                        _ => ToolStatus::Pending,
                    },
                })
            })
            .collect();
        if let Some(Item::Plan {
            steps: previous, ..
        }) = self.plan.and_then(|i| self.items.get_mut(i))
        {
            *previous = steps;
        } else {
            self.plan = Some(self.items.len());
            self.items.push(Item::Plan {
                key: format!("e{id}"),
                steps,
            });
        }
    }

    /// Settles the call a `toolResult` message answers.
    fn result(&mut self, message: &Value) {
        let Some(&i) = str_at(message, "toolCallId").and_then(|id| self.calls.get(id)) else {
            return;
        };
        let Some(Item::Tool(tool)) = self.items.get_mut(i) else {
            return;
        };
        tool.status = if message.get("isError").and_then(Value::as_bool) == Some(true) {
            ToolStatus::Failed
        } else {
            ToolStatus::Done
        };
        tool.output = cap_head(
            content_text(message.get("content")).trim_end(),
            OUTPUT_CHARS,
        );
        tool.duration_ms = message.get("durationMs").and_then(Value::as_u64);
        tool.child = tool.child.or_else(|| child(message.get("details")));
    }

    /// Marks the current round's calls with their live progress.
    fn live_tools(&mut self, slots: &[Value]) {
        for slot in slots {
            let Some(&i) = str_at(slot, "callId").and_then(|id| self.calls.get(id)) else {
                continue;
            };
            let Some(Item::Tool(tool)) = self.items.get_mut(i) else {
                continue;
            };
            tool.child = tool.child.or_else(|| child(slot.get("details")));
            if matches!(tool.status, ToolStatus::Done | ToolStatus::Failed) {
                continue;
            }
            match str_at(slot, "status") {
                Some("running") => tool.status = ToolStatus::Running,
                Some("pending") => tool.status = ToolStatus::Pending,
                _ => {}
            }
            if let Some(output) = str_at(slot, "output") {
                tool.output = cap_tail(output.trim_end(), OUTPUT_CHARS);
            }
        }
    }
}

/// Pushes an assistant message's parts: text, thinking, tool calls (with
/// `status` until their results arrive), and why it ended without an answer.
fn assistant(key: &str, message: &Value, items: &mut Vec<Item>, status: ToolStatus) {
    let parts = array(message.get("content"));
    let duration_ms = message.get("durationMs").and_then(Value::as_u64);
    let output: usize = parts.iter().map(output_len).sum();
    for (p, part) in parts.iter().enumerate() {
        let key = format!("{key}-{p}");
        match str_at(part, "type") {
            Some("text") => {
                let text = str_at(part, "text").unwrap_or_default();
                if !text.trim().is_empty() {
                    items.push(Item::Text {
                        key,
                        text: text.to_owned(),
                    });
                }
            }
            Some("thinking") => {
                let text = str_at(part, "thinking").unwrap_or_default();
                let redacted = part.get("redacted").and_then(Value::as_bool) == Some(true);
                if redacted || !text.trim().is_empty() {
                    items.push(Item::Thinking {
                        key,
                        text: text.to_owned(),
                        duration_ms: duration_ms.map(|ms| share(ms, output_len(part), output)),
                    });
                }
            }
            Some("toolCall") => items.push(Item::Tool(Box::new(tool_call(key, part, status)))),
            _ => {}
        }
    }
    let notice = match str_at(message, "stopReason") {
        Some("error") => Some(
            str_at(message, "errorMessage")
                .map_or_else(|| "The model returned an error".to_owned(), str::to_owned),
        ),
        Some("aborted") => Some("Answer aborted".to_owned()),
        _ => None,
    };
    if let Some(text) = notice {
        items.push(Item::Notice {
            key: format!("{key}-end"),
            text,
        });
    }
}

fn text_index(key: &str) -> Option<usize> {
    key.rsplit_once(':')?.1.parse().ok()
}

const fn acp_text(key: String, text: String, thinking: bool) -> Item {
    if thinking {
        Item::Thinking {
            key,
            text,
            duration_ms: None,
        }
    } else {
        Item::Text { key, text }
    }
}

impl Tool {
    fn from_acp(turn: Option<&str>, record: ToolRecord) -> Self {
        let name = match record.name.as_deref() {
            Some(
                "bash" | "read" | "read_file" | "write" | "write_file" | "edit" | "edit_file"
                | "apply_patch" | "grep" | "glob",
            ) => record.name,
            _ => record.kind.filter(|kind| kind != "other").or(record.name),
        }
        .unwrap_or_else(|| "tool".to_owned());
        let arguments = record
            .input
            .as_deref()
            .and_then(|input| serde_json::from_str(input).ok());
        let (summary, mut input, timeout_ms) = tool_input(&name, arguments.as_ref());
        if input.is_none() && is_command(&name) {
            let plain = record
                .input
                .as_deref()
                .filter(|text| arguments.is_none() && !text.trim_start().starts_with(['{', '[']));
            input = plain
                .or_else(|| record.title.strip_prefix("$ "))
                .map(|text| cap_head(text, ARGUMENTS_CHARS));
        }
        let status = match record.status {
            ToolCallStatus::Pending => ToolStatus::Pending,
            ToolCallStatus::InProgress => ToolStatus::Running,
            ToolCallStatus::Completed => ToolStatus::Done,
            ToolCallStatus::Failed => ToolStatus::Failed,
        };
        let summary = if summary.is_empty() {
            let title = if record.title == record.id {
                ""
            } else {
                &record.title
            };
            first_line(
                input
                    .as_deref()
                    .or_else(|| (!title.is_empty()).then_some(title))
                    .or_else(|| record.locations.first().map(String::as_str))
                    .unwrap_or_default(),
                SUMMARY_CHARS,
            )
        } else {
            summary
        };
        Self {
            key: format!("acp-{}-{}", turn.unwrap_or_default(), record.id),
            call_id: record.id,
            name,
            summary,
            input,
            timeout_ms,
            started_at: record.started_at,
            status,
            output: record.output.unwrap_or_default(),
            duration_ms: None,
            child: record.child,
            viz: record.viz,
            diffs: record.diffs,
            locations: record.locations,
        }
    }
}

fn tool_call(key: String, part: &Value, status: ToolStatus) -> Tool {
    let name = str_at(part, "name").unwrap_or("tool").to_owned();
    let (summary, input, timeout_ms) = tool_input(&name, part.get("arguments"));
    Tool {
        key,
        call_id: str_at(part, "id").unwrap_or_default().to_owned(),
        summary,
        input,
        timeout_ms,
        started_at: None,
        name,
        status,
        output: String::new(),
        duration_ms: None,
        child: None,
        viz: None,
        diffs: Vec::new(),
        locations: Vec::new(),
    }
}

pub fn is_command(name: &str) -> bool {
    matches!(name, "bash" | "execute" | "shell" | "terminal")
}

/// Project tool-specific fields out of native and ACP arguments. Unknown
/// structure stays in the transcript record, not in the presentation.
fn tool_input(name: &str, arguments: Option<&Value>) -> (String, Option<String>, Option<u64>) {
    let Some(args) = arguments else {
        return (String::new(), None, None);
    };
    let field = |keys: &[&str]| keys.iter().find_map(|key| str_at(args, key));
    let command = field(&["command", "cmd"]).or_else(|| {
        if is_command(name) {
            args.as_str()
        } else {
            None
        }
    });
    let summary = command
        .or_else(|| {
            field(&[
                "path",
                "file_path",
                "filePath",
                "task",
                "query",
                "pattern",
                "url",
                "prompt",
                "i",
            ])
        })
        .filter(|text| !text.starts_with("xd://mcp__viz_"))
        .or_else(|| args.as_str())
        .map_or_else(String::new, |text| first_line(text, SUMMARY_CHARS));
    let input = command
        .or_else(|| {
            if matches!(
                name,
                "write" | "create" | "edit" | "apply_patch" | "write_file" | "edit_file"
            ) {
                field(&["content", "patch", "newText", "new_string"])
            } else {
                None
            }
        })
        .map(|text| cap_head(text, ARGUMENTS_CHARS));
    let millis = |key| args.get(key).and_then(Value::as_u64);
    let timeout = millis("timeout_ms")
        .or_else(|| millis("timeoutMs"))
        .or_else(|| millis("timeout").and_then(|n| n.checked_mul(1_000)))
        .filter(|ms| *ms > 0);
    (summary, input, timeout)
}

/// How much output a part is, in characters, to apportion its message's time.
fn output_len(part: &Value) -> usize {
    let len = |name| str_at(part, name).map_or(0, str::len);
    match str_at(part, "type") {
        Some("text") => len("text"),
        Some("thinking") => len("thinking"),
        Some("toolCall") => part.get("arguments").map_or(0, |a| a.to_string().len()),
        _ => 0,
    }
}

/// `part` of `whole` of `ms`: tokens stream at a roughly steady rate, so a
/// part's share of the characters is about its share of the time.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "an approximate duration in whole milliseconds"
)]
fn share(ms: u64, part: usize, whole: usize) -> u64 {
    if whole == 0 {
        return ms;
    }
    (ms as f64 * part as f64 / whole as f64).round() as u64
}

impl Live {
    fn read(live: &Value, busy: bool) -> Self {
        let mut partial = Vec::new();
        if busy && let Some(message) = live.pointer("/generation/message") {
            assistant("live", message, &mut partial, ToolStatus::Pending);
        }
        let failure = live.get("runFailure").map(|failure| {
            let reason = str_at(failure, "reason")
                .unwrap_or("unknown")
                .replace('_', " ");
            let detail = str_at(failure, "detail").map_or_default(|detail| format!(": {detail}"));
            format!("Run ended without an answer ({reason}){detail}")
        });
        Self {
            busy,
            partial,
            retry: live
                .pointer("/generation/retry/error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            failure: failure.filter(|_| !busy),
            compacting: !array(live.get("compactions")).is_empty(),
        }
    }
}

/// A tool result's or progress `details.conversationId`.
fn child(details: Option<&Value>) -> Option<ConversationId> {
    details?.get("conversationId")?.as_u64().map(ConversationId)
}

/// A message's text: a string, or its text blocks joined.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match str_at(part, "type") {
                Some("text") => str_at(part, "text").map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The first non-empty line of `text`, cut to `max` characters, with an
/// ellipsis when anything was left out.
fn first_line(text: &str, max: usize) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let cut = cap_head(lines.next().unwrap_or_default(), max);
    if lines.next().is_some() && !cut.ends_with('…') {
        format!("{cut} …")
    } else {
        cut
    }
}

/// `text`'s first `max` characters, with an ellipsis if it was longer.
fn cap_head(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

/// `text`'s last `max` characters, with an ellipsis if it was longer.
fn cap_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let start = text
        .char_indices()
        .nth(count - max)
        .map_or(0, |(start, _)| start);
    format!("…{}", &text[start..])
}

fn array(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn str_at<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}
