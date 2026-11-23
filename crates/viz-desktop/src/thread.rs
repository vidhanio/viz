//! Renderable transcript and live ACP activity.

use std::borrow::Cow;

use serde_json::Value;
use viz_client::protocol::{
    Body, Entry, EntryData, EntryId, LiveTurn, PlanContent, PlanStatus, RunEnd, ThreadId, ToolCall,
    ToolCallStatus, ToolDiff,
};

/// Longest tool-call summary, in characters.
const SUMMARY_CHARS: usize = 96;
/// Longest command text shown in a tooltip, in characters.
const COMMAND_CHARS: usize = 4_000;
/// Longest tool output an expanded row shows, in characters (its tail for
/// running output, its head for a result).
const OUTPUT_CHARS: usize = 6_000;

/// A thread’s transcript window and live activity.
#[derive(Debug, Clone, Default)]
pub struct Thread {
    /// Timeline items in transcript order.
    pub items: Vec<Item>,
    /// Exclusive cursor for the next page of older entries.
    pub before: Option<EntryId>,
    plan: Option<usize>,
    ended_run: bool,
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
    },
    /// A tool call and, once settled, its result.
    Tool(Box<Tool>),
    /// The latest checklist reported in this ACP turn.
    Plan { key: String, steps: Vec<PlanStep> },
    /// The agent resumed work on its own.
    Continued { key: String },
    /// A non-default ACP stop reason shown in the activity timeline.
    TurnStatus { key: String, text: String },
    /// A delegated task's outcome, delivered back to the thread that
    /// delegated it.
    TaskResult(TaskResult),
    /// An assistant reply that ended without an answer.
    Notice {
        /// Stable element key.
        key: String,
        /// Why.
        text: String,
    },
    /// A settled run boundary carrying copyable reply text.
    TurnEnd(TurnEnd),
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

/// A settled run boundary carrying the assistant prose produced in that run
/// for one-click copy.
#[derive(Debug, Clone)]
pub struct TurnEnd {
    /// Stable element key.
    pub key: String,
    /// Assistant prose produced in this turn, for one-click copy.
    pub answer: String,
}

/// A delegated task's outcome, from the server's delivery text: `Task <id>
/// (<title>) on <place> <status>.`, then a blank line and the task's result or
/// error. Only the title, status and detail are shown; the rest are wire
/// details.
#[derive(Debug, Clone)]
pub struct TaskResult {
    /// Stable element key.
    pub key: String,
    /// The task's id, to find it among the thread's tasks.
    pub task: String,
    pub title: String,
    /// `completed`, `failed` or `cancelled`.
    pub status: String,
    /// The task's result or error.
    pub detail: String,
}

impl TaskResult {
    fn parse(key: String, task: &str, text: &str) -> Self {
        let (head, detail) = text.split_once("\n\n").unwrap_or((text, ""));
        let head = head.trim().trim_end_matches('.');
        let title = head
            .split_once(" (")
            .and_then(|(_, rest)| rest.rsplit_once(") on "))
            .map_or("", |(title, _)| title);
        let status = head.rsplit_once(' ').map_or("", |(_, status)| status);
        Self {
            key,
            task: task.to_owned(),
            title: title.to_owned(),
            status: status.to_owned(),
            detail: detail.trim().to_owned(),
        }
    }
}

impl Item {
    pub fn key(&self) -> &str {
        match self {
            Self::User(user) => &user.key,
            Self::Tool(tool) => &tool.key,
            Self::TaskResult(result) => &result.key,
            Self::TurnEnd(end) => &end.key,
            Self::Text { key, .. }
            | Self::Thinking { key, .. }
            | Self::Plan { key, .. }
            | Self::Continued { key }
            | Self::TurnStatus { key, .. }
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
    entry: Option<EntryId>,
    /// Tool name.
    pub name: String,
    /// One-line summary of the arguments.
    pub summary: String,
    /// Recognized command text, never a serialized arguments object.
    pub input: Option<String>,
    pub timeout_ms: Option<u64>,
    pub started_at: Option<i64>,
    /// Progress.
    pub status: ToolStatus,
    /// Exit status reported by a recognized terminal envelope.
    pub exit_code: Option<i32>,
    /// Display-ready result text or running output, capped.
    pub output: String,
    /// The task confirmed by a viz delegation receipt, even if it is still
    /// queued.
    pub task_id: Option<String>,
    /// The thread a subagent call delegated to.
    pub child: Option<ThreadId>,
    pub child_environment: Option<String>,
    /// The viz tool an ACP call ran.
    pub viz: Option<String>,
    pub diffs: Vec<ToolDiff>,
}

#[derive(Debug, Clone)]
pub struct PlanStep {
    pub text: String,
    pub status: ToolStatus,
}

impl Thread {
    /// Match the separately watched ACP state to the prompt in the transcript.
    pub fn sync_acp(&mut self, live: Option<&LiveTurn>) {
        self.acp_partial.clear();
        let Some(live) = live.filter(|live| {
            !self.ended_run && self.acp_turn.as_ref().is_none_or(|turn| turn == &live.turn)
        }) else {
            return;
        };
        for entry in &live.entries {
            match &entry.body {
                Body::Text(text) | Body::Thought(text)
                    if self.acp_text_index.is_none_or(|committed| {
                        text_index(&entry.key).is_some_and(|index| index > committed)
                    }) =>
                {
                    let settled = self
                        .items
                        .last()
                        .filter(|_| self.items.len() > self.acp_start);
                    let thinking = matches!(&entry.body, Body::Thought(_));
                    let preceding = match (thinking, settled) {
                        (false, Some(Item::Text { key, text }))
                        | (true, Some(Item::Thinking { key, text, .. })) => Some((key, text)),
                        _ => None,
                    };
                    let (key, text) = preceding.map_or_else(
                        || (entry.key.clone(), text.text.clone()),
                        |(key, earlier)| (key.clone(), format!("{earlier}{}", text.text)),
                    );
                    self.acp_partial.push(acp_text(key, text, thinking));
                }
                Body::Tool(record) => {
                    let committed = self.items[self.acp_start..]
                        .iter()
                        .any(|item| matches!(item, Item::Tool(tool) if tool.call_id == record.id));
                    if !committed {
                        self.acp_partial.push(Item::Tool(Box::new(Tool::from_acp(
                            Some(&live.turn),
                            record,
                        ))));
                    }
                }
                _ => {}
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

    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = &'a Entry>,
        before: Option<EntryId>,
    ) -> Self {
        let mut thread = Self {
            before,
            ..Self::default()
        };
        thread.append(entries);
        thread
    }

    pub fn append<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>) {
        for entry in entries {
            self.entry(entry);
        }
    }

    fn entry(&mut self, Entry { id, data }: &Entry) {
        match data {
            EntryData::Prompt {
                text, attachments, ..
            } => {
                if !text.trim().is_empty() || !attachments.is_empty() {
                    self.items.push(Item::User(User {
                        key: format!("e{id}-0"),
                        text: text.clone(),
                        attachments: attachments.iter().map(|file| file.name.clone()).collect(),
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
                self.items.push(Item::TaskResult(TaskResult::parse(
                    format!("e{id}"),
                    task,
                    text,
                )));
                self.start(*id);
            }
            EntryData::ToolUpdate { entry, record } => self.update_tool(*entry, record),
            EntryData::Transcript(entry) => match &entry.body {
                Body::Text(segment) => self.commit_text(&entry.key, &segment.text, false),
                Body::Thought(segment) => self.commit_text(&entry.key, &segment.text, true),
                Body::Tool(record) => {
                    let mut tool = Tool::from_acp(self.acp_turn.as_deref(), record);
                    tool.entry = Some(*id);
                    self.items.push(Item::Tool(Box::new(tool)));
                }
                Body::Plan(plan) => {
                    let PlanContent::Items { entries } = &plan.content else {
                        return;
                    };
                    let steps = entries
                        .iter()
                        .map(|step| PlanStep {
                            text: step.content.clone(),
                            status: match step.status {
                                PlanStatus::Completed => ToolStatus::Done,
                                PlanStatus::InProgress => ToolStatus::Running,
                                PlanStatus::Pending => ToolStatus::Pending,
                            },
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
                Body::Media(_) | Body::Compaction(_) | Body::Omitted => {}
            },
            EntryData::Omitted => {}
            EntryData::Run(end) => self.finish_run(*id, end),
        }
    }

    fn finish_run(&mut self, id: EntryId, end: &RunEnd) {
        let mut prose_parts = Vec::new();
        for item in &mut self.items[self.acp_start..] {
            match item {
                Item::Tool(tool)
                    if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) =>
                {
                    tool.status = ToolStatus::Abandoned;
                }
                Item::Text { text, .. } if !text.trim().is_empty() => {
                    prose_parts.push(text.clone());
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
                text: text.clone(),
            });
        } else if let Some(reason) = end
            .stop_reason
            .as_deref()
            .filter(|reason| !matches!(*reason, "end_turn" | "steered"))
        {
            let text = match reason {
                "cancelled" => "Stopped".to_owned(),
                "max_tokens" => "Reached the token limit".to_owned(),
                "max_turn_requests" => "Reached the request limit for this run".to_owned(),
                "refusal" => "The agent declined to continue".to_owned(),
                _ => format!("Run ended: {}", reason.replace('_', " ")),
            };
            self.items.push(Item::TurnStatus {
                key: format!("e{id}"),
                text,
            });
        }
        let answer = if prose_parts.is_empty() {
            end.answer.clone().unwrap_or_default()
        } else {
            prose_parts.join("\n\n")
        };
        if !answer.is_empty() {
            self.items.push(Item::TurnEnd(TurnEnd {
                key: format!("e{id}-end"),
                answer,
            }));
        }
    }

    fn update_tool(&mut self, entry: EntryId, record: &ToolCall) {
        if let Some(tool) = self.items.iter_mut().find_map(|item| match item {
            Item::Tool(tool) if tool.entry == Some(entry) => Some(tool),
            _ => None,
        }) {
            let mut updated = Tool::from_acp(None, record);
            updated.key.clone_from(&tool.key);
            updated.entry = Some(entry);
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

    fn commit_text(&mut self, key: &str, text: &str, thinking: bool) {
        self.acp_text_index = text_index(key);
        if self.items.len() > self.acp_start {
            match (thinking, self.items.last_mut()) {
                (true, Some(Item::Thinking { text: previous, .. }))
                | (false, Some(Item::Text { text: previous, .. })) => {
                    previous.push_str(text);
                    return;
                }
                _ => {}
            }
        }
        self.items
            .push(acp_text(key.to_owned(), text.to_owned(), thinking));
    }
}

fn text_index(key: &str) -> Option<usize> {
    key.rsplit_once(':')?.1.parse().ok()
}

const fn acp_text(key: String, text: String, thinking: bool) -> Item {
    if thinking {
        Item::Thinking { key, text }
    } else {
        Item::Text { key, text }
    }
}

impl Tool {
    fn from_acp(turn: Option<&str>, record: &ToolCall) -> Self {
        let name = match record.name.as_deref() {
            Some(
                "bash" | "read" | "read_file" | "write" | "write_file" | "edit" | "edit_file"
                | "apply_patch" | "grep" | "glob",
            ) => record.name.as_deref(),
            _ => record
                .kind
                .as_deref()
                .filter(|kind| *kind != "other")
                .or(record.name.as_deref()),
        }
        .unwrap_or("tool");
        let command = is_command(name);
        let plain_input = record
            .raw_input
            .as_ref()
            .and_then(serde_json::Value::as_str);
        let parsed = plain_input.and_then(|input| serde_json::from_str(input).ok());
        let arguments = parsed
            .as_ref()
            .or_else(|| record.raw_input.as_ref().filter(|value| value.is_object()));
        let (summary, mut input, timeout_ms) = tool_input(name, arguments);
        if input.is_none() && command {
            input = plain_input
                .filter(|text| arguments.is_none() && !text.trim_start().starts_with(['{', '[']))
                .or_else(|| record.title.strip_prefix("$ "))
                .map(|text| cap_head(text, COMMAND_CHARS));
        }
        let rendered = record.output_text();
        let output = rendered.as_deref().unwrap_or_default();
        let delegation = record
            .viz
            .as_ref()
            .is_some_and(|viz| viz.tool == "delegate_task");
        let (output, legacy_exit) = if command || delegation {
            antigravity_output(output, record.status).unwrap_or((output, None))
        } else {
            (output, None)
        };
        let exit_code = record
            .exit
            .as_ref()
            .and_then(|exit| exit.code)
            .and_then(|code| i32::try_from(code).ok())
            .or(legacy_exit);
        let preserve_output = command || matches!(name, "read" | "read_file");
        let (output, task_id) = readable_output(output, preserve_output, delegation);
        let status = match record.status {
            ToolCallStatus::Pending => ToolStatus::Pending,
            ToolCallStatus::InProgress => ToolStatus::Running,
            ToolCallStatus::Completed if exit_code.is_some_and(|code| code != 0) => {
                ToolStatus::Failed
            }
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
                    .or_else(|| {
                        record
                            .locations
                            .first()
                            .and_then(|location| location.path.to_str())
                    })
                    .unwrap_or_default(),
                SUMMARY_CHARS,
            )
        } else {
            summary
        };
        Self {
            key: format!("acp-{}-{}", turn.unwrap_or_default(), record.id),
            call_id: record.id.clone(),
            entry: None,
            name: name.to_owned(),
            summary,
            input,
            timeout_ms,
            started_at: record.started_at,
            status,
            exit_code,
            output: cap_output(output, status),
            task_id,
            child: record.viz.as_ref().and_then(|viz| viz.child),
            child_environment: record.viz.as_ref().and_then(|viz| viz.environment.clone()),
            viz: record.viz.as_ref().map(|viz| viz.tool.clone()),
            diffs: record.diffs().cloned().collect(),
        }
    }
}

pub fn is_command(name: &str) -> bool {
    matches!(name, "bash" | "execute" | "shell" | "terminal")
}

/// Antigravity sends the model-facing terminal envelope as ACP content:
/// <https://github.com/google-antigravity/antigravity-cli/issues/1506>
fn antigravity_output(output: &str, status: ToolCallStatus) -> Option<(&str, Option<i32>)> {
    let (timestamp, rest) = output_line(output.strip_prefix("Created At: ")?);
    if !terminal_timestamp(timestamp) {
        return None;
    }
    let (line, tail) = output_line(rest);
    let rest = if line
        .strip_prefix("Completed At: ")
        .is_some_and(terminal_timestamp)
    {
        tail
    } else {
        rest
    };
    let rest = rest.trim_start_matches(['\n', '\r']);
    let (line, tail) = output_line(rest);
    let exit_code = line
        .strip_prefix("Command exited with code ")
        .and_then(|code| code.strip_suffix('.'))
        .and_then(|code| code.parse().ok());
    let rest = if exit_code.is_some() {
        tail.trim_start_matches(['\n', '\r'])
    } else {
        rest
    };
    let (heading, body) = output_line(rest);
    if heading == "Command output:" {
        return Some((body, exit_code));
    }
    let unfinished = matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress);
    (rest.is_empty() && (exit_code.is_some() || unfinished)).then_some(("", exit_code))
}

fn output_line(text: &str) -> (&str, &str) {
    let (line, rest) = text.split_once('\n').unwrap_or((text, ""));
    (line.strip_suffix('\r').unwrap_or(line), rest)
}

fn terminal_timestamp(timestamp: &str) -> bool {
    let Some((date_time, zone)) = timestamp.split_at_checked(19) else {
        return false;
    };
    let matches_format = |text: &str, shape: &[u8]| {
        text.len() == shape.len()
            && text
                .bytes()
                .zip(shape)
                .all(|(byte, &expected)| match expected {
                    b'#' => byte.is_ascii_digit(),
                    _ => byte == expected,
                })
    };
    matches_format(date_time, b"####-##-##T##:##:##")
        && (zone == "Z"
            || zone
                .strip_prefix(['+', '-'])
                .is_some_and(|offset| matches_format(offset, b"##:##")))
}

/// Keep prose and known result fields, not opaque RPC objects. A confirmed
/// delegation can get its task ID from the same result before it is hidden.
fn readable_output(
    output: &str,
    preserve_output: bool,
    delegation: bool,
) -> (Cow<'_, str>, Option<String>) {
    if (preserve_output && !delegation) || !output.trim_start().starts_with(['{', '[']) {
        return (Cow::Borrowed(output), None);
    }
    let mut parsed = serde_json::from_str::<Value>(output);
    let task_id = if delegation {
        let id = parsed
            .as_mut()
            .ok()
            .and_then(|value| value.get_mut("task")?.get_mut("id"));
        match id.map(Value::take) {
            Some(Value::String(id)) => Some(id),
            _ => None,
        }
    } else {
        None
    };
    let text = if preserve_output {
        Cow::Borrowed(output)
    } else {
        match parsed {
            Ok(value) => Cow::Owned(json_text(value).unwrap_or_default()),
            Err(error) if error.is_eof() => Cow::Borrowed(""),
            Err(_) => Cow::Borrowed(output),
        }
    };
    (text, task_id)
}

fn json_text(value: Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.trim().is_empty()).then_some(text),
        Value::Object(mut fields) => ["text", "message", "error", "output", "content"]
            .into_iter()
            .find_map(|key| fields.remove(key).and_then(json_text)),
        Value::Array(items) => items
            .into_iter()
            .filter_map(json_text)
            .reduce(|mut text, next| {
                text.push('\n');
                text.push_str(&next);
                text
            }),
        _ => None,
    }
}

/// Project tool-specific fields out of ACP arguments. Unknown
/// structure stays in the transcript record, not in the presentation.
fn tool_input(name: &str, arguments: Option<&Value>) -> (String, Option<String>, Option<u64>) {
    let Some(args) = arguments else {
        return (String::new(), None, None);
    };
    let field = |keys: &[&str]| keys.iter().find_map(|key| str_at(args, key));
    let command = if is_command(name) {
        field(&[
            "command",
            "cmd",
            "CommandLine",
            "commandLine",
            "command_line",
        ])
        .or_else(|| args.as_str())
    } else {
        None
    };
    let summary = command
        .or_else(|| {
            field(&[
                "path",
                "file_path",
                "filePath",
                "absolute_path",
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
    let input = command.map(|text| cap_head(text, COMMAND_CHARS));
    let millis = |key| args.get(key).and_then(Value::as_u64);
    let timeout = millis("timeout_ms")
        .or_else(|| millis("timeoutMs"))
        .or_else(|| millis("timeout").and_then(|n| n.checked_mul(1_000)))
        .filter(|ms| *ms > 0);
    (summary, input, timeout)
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

fn cap_output(text: Cow<'_, str>, status: ToolStatus) -> String {
    let live = matches!(status, ToolStatus::Running | ToolStatus::Pending);
    let cut = if live {
        text.char_indices()
            .rev()
            .nth(OUTPUT_CHARS)
            .map(|(cut, ch)| cut + ch.len_utf8())
    } else {
        text.char_indices().nth(OUTPUT_CHARS).map(|(cut, _)| cut)
    };
    let Some(cut) = cut else {
        return text.into_owned();
    };
    match text {
        Cow::Borrowed(text) if live => format!("…{}", &text[cut..]),
        Cow::Borrowed(text) => format!("{}…", &text[..cut]),
        Cow::Owned(mut text) => {
            let range = if live { 0..cut } else { cut..text.len() };
            text.replace_range(range, "…");
            text
        }
    }
}

fn str_at<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}
