//! Pure transcript projection and live ACP turn folding.

use viz_client::protocol::{
    Body, Entry, EntryData, EntryId, LiveTurn, PlanContent, PlanStatus, RunEnd, ThreadId, ToolCall,
    ToolDiff,
};

use super::tool_parse::parse_tool;

/// A thread's transcript window and live uncommitted ACP tail.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    /// Committed timeline items in transcript order.
    pub items: Vec<Item>,
    /// Exclusive cursor for the next page of older entries.
    pub before: Option<EntryId>,
    plan: Option<usize>,
    ended_run: bool,
    acp_turn: Option<String>,
    acp_start: usize,
    acp_text_index: Option<usize>,
    /// Live streaming text and unfinished tool calls for the active run.
    pub acp_partial: Vec<Item>,
}

/// One timeline row.
#[derive(Debug, Clone)]
pub enum Item {
    /// Something the user wrote.
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
    Plan {
        /// Stable element key.
        key: String,
        /// Plan checklist entries in order.
        steps: Vec<PlanStep>,
    },
    /// The agent resumed work on its own.
    Continued {
        /// Stable element key.
        key: String,
    },
    /// A non-default ACP stop reason shown in the activity timeline.
    TurnStatus {
        /// Stable element key.
        key: String,
        /// Human-readable stop reason.
        text: String,
    },
    /// A delegated task's outcome, delivered back to the thread that
    /// delegated it.
    TaskResult(TaskResult),
    /// An assistant reply that ended without an answer.
    Notice {
        /// Stable element key.
        key: String,
        /// Why the reply ended with an error.
        text: String,
    },
    /// A settled run boundary carrying copyable reply text.
    TurnEnd(TurnEnd),
}

/// Something the user wrote. Single-user: every input is the owner's,
/// whichever client or connector it came through.
#[derive(Debug, Clone)]
pub struct User {
    /// Stable element key.
    pub key: String,
    /// What the user wrote.
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
    /// Task title extracted from the delivery header.
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
    /// Stable element key for this timeline row.
    #[must_use]
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
    pub(super) entry: Option<EntryId>,
    /// Tool name.
    pub name: String,
    /// One-line summary of the arguments.
    pub summary: String,
    /// Recognized command text, never a serialized arguments object.
    pub input: Option<String>,
    /// Command timeout in milliseconds, when advertised.
    pub timeout_ms: Option<u64>,
    /// Unix milliseconds when the tool started running.
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
    /// Target environment for the delegated child thread, if remote.
    pub child_environment: Option<String>,
    /// The viz tool an ACP call ran.
    pub viz: Option<String>,
    /// File diffs reported by the tool call.
    pub diffs: Vec<ToolDiff>,
}

/// One checklist step in an ACP plan entry.
#[derive(Debug, Clone)]
pub struct PlanStep {
    /// Step description.
    pub text: String,
    /// Step completion status.
    pub status: ToolStatus,
}

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
                        self.acp_partial
                            .push(Item::Tool(Box::new(parse_tool(Some(&live.turn), record))));
                    }
                }
                _ => {}
            }
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
                    let mut tool = parse_tool(self.acp_turn.as_deref(), record);
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
            let mut updated = parse_tool(None, record);
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
