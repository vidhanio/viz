//! The watched conversation as the window shows it, read from a pi-durable
//! `ConversationView` (`value.entries` and `value.docs`). Shapes are
//! pi-durable's `EntryRecord`, pi-ai messages and live document;
//! an unexpected entry is skipped rather than failing the whole view.

use std::collections::HashMap;

use serde_json::Value;
use viz_client::protocol::{ConversationId, EntryId};

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
    Tool(Tool),
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
    /// Prompt entry used by the turn diff and revert RPCs.
    pub turn: Option<EntryId>,
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
    /// The arguments, pretty-printed and capped.
    pub arguments: String,
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

#[derive(Debug, Clone, Hash)]
pub struct ToolDiff {
    pub path: String,
    pub old: Option<String>,
    pub new: String,
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
        };
        let mut acp_busy = false;
        for entry in earlier.iter().chain(array(view.get("entries"))) {
            match str_at(entry, "kind") {
                Some("viz.agent.prompt") => acp_busy = true,
                Some("viz.agent.turn") => {
                    acp_busy = false;
                    reader.plan = None;
                }
                _ => {}
            }
            reader.entry(entry);
        }
        reader.live_tools(slots);
        let native_busy = live.is_some_and(|live| live.get("run").is_some());
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
            "pi.user" | "viz.input" | "viz.agent.prompt" => {
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
        let turn = entry.get("id").and_then(Value::as_u64).map(EntryId);
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
                    turn,
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
                    turn,
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
                match (kind, self.items.last_mut()) {
                    ("viz.agent.text", Some(Item::Text { text: previous, .. }))
                    | ("viz.agent.thought", Some(Item::Thinking { text: previous, .. })) => {
                        previous.push_str(text);
                    }
                    ("viz.agent.text", _) => self.items.push(Item::Text {
                        key: format!("e{id}"),
                        text: text.to_owned(),
                    }),
                    _ => self.items.push(Item::Thinking {
                        key: format!("e{id}"),
                        text: text.to_owned(),
                        duration_ms: None,
                    }),
                }
            }
            "viz.agent.tool" => {
                let Some(data) = entry.get("data") else {
                    return;
                };
                let title = str_at(data, "title").unwrap_or("Tool");
                let status = match str_at(data, "status") {
                    Some("pending") => ToolStatus::Pending,
                    Some("in_progress") => ToolStatus::Running,
                    Some("completed") => ToolStatus::Done,
                    Some("failed") => ToolStatus::Failed,
                    _ => ToolStatus::Abandoned,
                };
                self.items.push(Item::Tool(Tool {
                    key: format!("e{id}"),
                    call_id: str_at(data, "id").unwrap_or_default().to_owned(),
                    name: str_at(data, "kind").unwrap_or("tool").to_owned(),
                    summary: first_line(title, SUMMARY_CHARS),
                    arguments: str_at(data, "input").unwrap_or_default().to_owned(),
                    status,
                    output: str_at(data, "output").unwrap_or_default().to_owned(),
                    duration_ms: None,
                    child: data
                        .get("child")
                        .and_then(Value::as_u64)
                        .map(ConversationId),
                    viz: str_at(data, "viz").map(str::to_owned),
                    diffs: array(data.get("diffs"))
                        .iter()
                        .filter_map(|diff| {
                            Some(ToolDiff {
                                path: str_at(diff, "path")?.to_owned(),
                                old: str_at(diff, "oldText").map(str::to_owned),
                                new: str_at(diff, "newText")?.to_owned(),
                            })
                        })
                        .collect(),
                    locations: array(data.get("locations"))
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                }));
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
            Some("toolCall") => items.push(Item::Tool(tool_call(key, part, status))),
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

fn tool_call(key: String, part: &Value, status: ToolStatus) -> Tool {
    let name = str_at(part, "name").unwrap_or("tool").to_owned();
    let arguments = part.get("arguments");
    Tool {
        key,
        call_id: str_at(part, "id").unwrap_or_default().to_owned(),
        summary: arguments.map_or_default(summary),
        arguments: arguments
            .and_then(|arguments| serde_json::to_string_pretty(arguments).ok())
            .map_or_default(|pretty| cap_head(&pretty, ARGUMENTS_CHARS)),
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

/// One line for a call's arguments: its main string argument when it has one
/// (a task, command, path or query), else `key: value` pairs.
fn summary(arguments: &Value) -> String {
    const MAIN: [&str; 8] = [
        "task", "command", "cmd", "path", "query", "pattern", "url", "prompt",
    ];
    let Value::Object(fields) = arguments else {
        return first_line(&arguments.to_string(), SUMMARY_CHARS);
    };
    if let Some(main) = MAIN
        .iter()
        .find_map(|name| fields.get(*name).and_then(Value::as_str))
    {
        return first_line(main, SUMMARY_CHARS);
    }
    if let (1, Some(Value::String(only))) = (fields.len(), fields.values().next()) {
        return first_line(only, SUMMARY_CHARS);
    }
    let pairs = fields
        .iter()
        .map(|(name, value)| match value {
            Value::String(s) => format!("{name}: {s}"),
            other => format!("{name}: {other}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    first_line(&pairs, SUMMARY_CHARS)
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
