//! The watched conversation as the window shows it, read from a pi-durable
//! `ConversationView` (`value.entries` and `value.docs`). Shapes are
//! pi-durable's `EntryRecord`, pi-ai's messages and the `pi.live`, `pi.agent`,
//! `pi.usage` and `pi.inbox` documents; anything unexpected is skipped rather
//! than failing the whole view.

use std::collections::HashMap;

use serde_json::Value;
use viz_client::protocol::ConversationId;

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
    /// The window starts after the transcript's first entry.
    pub earlier: bool,
    /// What's happening right now (`pi.live`).
    pub live: Live,
    /// Model and thinking level (`pi.agent`).
    pub agent: Agent,
    /// The conversation's own spend (`pi.usage`), once it has any.
    pub usage: Option<Usage>,
    /// Inputs waiting for the current run (`pi.inbox`).
    pub queued: usize,
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
    /// The conversation a `subagent` call delegated to.
    pub child: Option<ConversationId>,
}

/// `pi.live`, as the window shows it.
#[derive(Debug, Clone, Default)]
pub struct Live {
    /// A run is in progress (`run` present).
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

/// `pi.agent`'s model and thinking level.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    /// Provider id, e.g. `anthropic`.
    pub provider: Option<String>,
    /// Model id.
    pub model: Option<String>,
    /// Thinking level, e.g. `high`.
    pub thinking: Option<String>,
}

/// `pi.usage` summed over every model and tool bucket.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// Input tokens, cached or not.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Total cost in dollars.
    pub cost: f64,
}

impl Thread {
    /// Reads a conversation view's value.
    pub fn from_view(view: &Value) -> Self {
        let live = view.pointer("/docs/pi.live");
        let slots = live
            .and_then(|live| live.get("tools"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let mut reader = Reader {
            items: Vec::new(),
            calls: HashMap::new(),
        };
        for entry in array(view.get("entries")) {
            reader.entry(entry);
        }
        reader.live_tools(slots);
        let busy = live.is_some_and(|live| live.get("run").is_some());
        let mut items = reader.items;
        for item in &mut items {
            if let Item::Tool(tool) = item
                && !busy
                && matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
            {
                tool.status = ToolStatus::Abandoned;
            }
        }
        Self {
            items,
            earlier: view.get("before").is_some(),
            live: live.map_or_default(|live| Live::read(live, busy)),
            agent: view.pointer("/docs/pi.agent").map_or_default(Agent::read),
            usage: view.pointer("/docs/pi.usage").map(Usage::read),
            queued: array(view.pointer("/docs/pi.inbox/items"))
                .iter()
                .filter(|item| str_at(item, "mode") != Some("write"))
                .count(),
        }
    }

    /// The child conversations of its `subagent` calls.
    pub fn children(&self) -> impl Iterator<Item = ConversationId> + '_ {
        self.items.iter().filter_map(|item| match item {
            Item::Tool(tool) => tool.child,
            _ => None,
        })
    }
}

/// Builds items from entries in order, pairing tool results with their calls.
struct Reader {
    items: Vec<Item>,
    /// Item index of each tool call by call id.
    calls: HashMap<String, usize>,
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
            "pi.user" | "viz.input" => {
                // A connector's input reaches the model with a routing prefix;
                // `data.text` is what I actually wrote.
                let raw = entry
                    .pointer("/data/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let texts: Vec<String> = raw.map_or_else(
                    || {
                        messages
                            .iter()
                            .map(|message| content_text(message.get("content")))
                            .collect()
                    },
                    |raw| vec![raw],
                );
                for (m, text) in texts.into_iter().enumerate() {
                    if !text.trim().is_empty() {
                        self.items.push(Item::User(User {
                            key: format!("e{id}-{m}"),
                            text,
                        }));
                    }
                }
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
            _ => {}
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

impl Agent {
    fn read(agent: &Value) -> Self {
        Self {
            provider: agent
                .pointer("/model/provider")
                .and_then(Value::as_str)
                .map(str::to_owned),
            model: agent
                .pointer("/model/modelId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            thinking: str_at(agent, "thinkingLevel").map(str::to_owned),
        }
    }
}

impl Usage {
    fn read(usage: &Value) -> Self {
        let mut sum = Self::default();
        for bucket in ["models", "tools"] {
            let Some(Value::Object(entries)) = usage.get(bucket) else {
                continue;
            };
            for entry in entries.values() {
                sum.add(entry);
            }
        }
        sum
    }

    fn add(&mut self, usage: &Value) {
        let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
        self.input += count("input") + count("cacheRead") + count("cacheWrite");
        self.output += count("output");
        self.cost += usage
            .pointer("/cost/total")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    }

    /// Tokens in and out.
    pub const fn tokens(self) -> u64 {
        self.input + self.output
    }
}

/// A tool result's or progress `details.conversationId`.
fn child(details: Option<&Value>) -> Option<ConversationId> {
    details?.get("conversationId")?.as_u64()
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
