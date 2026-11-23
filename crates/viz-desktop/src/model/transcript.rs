//! Pure transcript data model: user prompts, assistant prose/reasoning,
//! rich tool records, plans, compactions, media, omitted entries, and run ends.

use viz_client::protocol::{
    Compaction, CompactionStatus, ContentBlock, EntryId, EntrySource, Origin, Plan, PlanContent,
    PlanPriority, PlanStatus, Review, Segment, ThreadId, ToolDiff, TranscriptEntry, TurnUsage,
};

/// A thread's transcript window and live uncommitted ACP tail.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    /// Committed timeline items in transcript order.
    pub items: Vec<Item>,
    /// Exclusive cursor for the next page of older entries.
    pub before: Option<EntryId>,
    pub(super) plan: Option<(Option<String>, usize)>,
    pub(super) ended_run: bool,
    pub(super) acp_turn: Option<String>,
    pub(super) acp_start: usize,
    pub(super) acp_text_index: Option<usize>,
    /// Live streaming text and unfinished tool calls for the active run.
    pub acp_partial: Vec<Item>,
}

/// Decoded full payload retrieved lazily via `thread.entry_chunk`.
#[derive(Debug, Clone)]
pub enum LoadedEntry {
    /// Decoded committed [`viz_client::protocol::EntryData`] row.
    Stored(Box<viz_client::protocol::EntryData>),
    /// Decoded live [`TranscriptEntry`] segment.
    Live(Box<TranscriptEntry>),
}

/// Semantic error severity for inline turn error banners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Harness reported a quota or rate limit (includes reset window when
    /// known).
    UsageLimit,
    /// Harness reported an authentication or sign-in error.
    Auth,
    /// General run or protocol failure.
    General,
}

impl ErrorKind {
    /// Classifies an error string into a banner tone and heading.
    #[must_use]
    pub fn classify(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        if lower.contains("usage limit")
            || lower.contains("rate limit")
            || lower.contains("5-hour limit")
            || lower.contains("quota")
            || lower.contains("too many requests")
        {
            Self::UsageLimit
        } else if lower.contains("not signed in")
            || lower.contains("authentication")
            || lower.contains("unauthorized")
            || lower.contains("api key")
            || lower.contains("sign in")
        {
            Self::Auth
        } else {
            Self::General
        }
    }

    /// Short human-readable heading for the inline error banner.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::UsageLimit => "Usage limit reached",
            Self::Auth => "Authentication required",
            Self::General => "Run failed",
        }
    }
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
        /// Wire entry id, when committed.
        entry_id: Option<EntryId>,
        /// The Markdown source.
        text: String,
        /// Unix ms of first chunk, when reported.
        started_at: Option<i64>,
        /// Unix ms of last chunk, when reported.
        ended_at: Option<i64>,
        /// True when truncated on the wire (`thread.entry_chunk` fetches more).
        more: bool,
    },
    /// The assistant's reasoning, collapsed inside the turn fold.
    Thinking {
        /// Stable element key.
        key: String,
        /// Wire entry id, when committed.
        entry_id: Option<EntryId>,
        /// The reasoning text (empty when redacted).
        text: String,
        /// Unix ms of first chunk, when reported.
        started_at: Option<i64>,
        /// Unix ms of last chunk, when reported.
        ended_at: Option<i64>,
        /// True when truncated on the wire.
        more: bool,
    },
    /// A tool call and, once settled, its result.
    Tool(Box<Tool>),
    /// A structured checklist or markdown plan reported by the harness.
    Plan(PlanCard),
    /// Context compaction notice reported by the harness.
    Compaction(CompactionNotice),
    /// Non-text media or embedded resource emitted by the agent.
    Media(MediaItem),
    /// Large omitted wire entry recoverable via `thread.entry_chunk`.
    Omitted {
        /// Stable element key.
        key: String,
        /// Source selector for lazy chunk loading.
        source: EntrySource,
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
        /// Raw wire stop reason identifier.
        stop_reason: String,
        /// Human-readable stop reason.
        text: String,
    },
    /// A delegated task's outcome, delivered back to the thread that
    /// delegated it.
    TaskResult(TaskResult),
    /// An assistant reply that ended with an error.
    Notice {
        /// Stable element key.
        key: String,
        /// Classified error kind.
        kind: ErrorKind,
        /// Why the reply ended with an error.
        text: String,
    },
    /// A settled run boundary carrying copyable reply text and usage telemetry.
    TurnEnd(TurnEnd),
}

impl Item {
    /// Stable element key for this timeline row.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::User(user) => &user.key,
            Self::Tool(tool) => &tool.key,
            Self::Plan(plan) => &plan.key,
            Self::Compaction(notice) => &notice.key,
            Self::Media(media) => &media.key,
            Self::TaskResult(result) => &result.key,
            Self::TurnEnd(end) => &end.key,
            Self::Text { key, .. }
            | Self::Thinking { key, .. }
            | Self::Omitted { key, .. }
            | Self::Continued { key }
            | Self::TurnStatus { key, .. }
            | Self::Notice { key, .. } => key,
        }
    }
}

/// Something the user wrote.
#[derive(Debug, Clone)]
pub struct User {
    /// Stable element key.
    pub key: String,
    /// Wire entry id, used for `thread.fork` (`Before` / `Through`).
    pub entry_id: Option<EntryId>,
    /// What the user wrote.
    pub text: String,
    /// Stored attachments carried by this prompt.
    pub attachments: Vec<viz_client::protocol::PromptAttachment>,
    /// Files and links mentioned with `@` or `#`.
    pub context: Vec<viz_client::protocol::PromptContext>,
    /// External connector origin when sent outside desktop.
    pub origin: Option<Origin>,
    /// Whether this prompt arrived while a prior run was active (steer).
    pub steered: bool,
}

/// A structured checklist or markdown plan card.
#[derive(Debug, Clone)]
pub struct PlanCard {
    /// Stable element key.
    pub key: String,
    /// Observation timestamp in Unix ms, when reported.
    pub at: Option<i64>,
    /// Checklist entries or markdown plan prose.
    pub content: PlanBody,
}

/// Payload of a [`PlanCard`].
#[derive(Debug, Clone)]
pub enum PlanBody {
    /// Ordered checklist steps with priority and progress.
    Items(Vec<PlanStep>),
    /// Proposed plan document in Markdown, optionally backed by a file path.
    Markdown {
        /// Markdown source.
        text: String,
        /// Optional workspace-relative or absolute plan file path.
        path: Option<String>,
    },
}

/// Context compaction event surfaced as a system divider.
#[derive(Debug, Clone)]
pub struct CompactionNotice {
    /// Stable element key.
    pub key: String,
    /// Harness compaction id.
    pub id: String,
    /// Live or settled compaction status.
    pub status: CompactionStatus,
    /// Human-readable summary kept by the harness.
    pub summary: String,
    /// Optional compaction failure detail.
    pub error: Option<String>,
    /// Start timestamp in Unix ms.
    pub started_at: i64,
    /// End timestamp in Unix ms.
    pub ended_at: i64,
}

/// Non-text agent media block (image, audio, resource link).
#[derive(Debug, Clone)]
pub struct MediaItem {
    /// Stable element key.
    pub key: String,
    /// ACP content block payload.
    pub block: ContentBlock,
}

/// A settled run boundary carrying copyable reply text, stop reason, and usage.
#[derive(Debug, Clone)]
pub struct TurnEnd {
    /// Stable element key.
    pub key: String,
    /// Wire entry id of the run-end record, for `ForkPoint::Through`.
    pub entry_id: Option<EntryId>,
    /// Assistant prose produced in this turn, for one-click copy.
    pub answer: String,
    /// Stop reason reported by the harness.
    pub stop_reason: Option<String>,
    /// Context usage, cost, and end-turn token telemetry.
    pub usage: Option<TurnUsage>,
    /// Earliest timestamp observed during this run, in Unix ms.
    pub started_at: Option<i64>,
    /// Latest timestamp observed during this run, in Unix ms.
    pub ended_at: Option<i64>,
}

/// A delegated task's outcome delivered back to its parent thread.
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
    pub(super) fn parse(key: String, task: &str, text: &str) -> Self {
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

/// One parsed search hit surfaced in the search tool inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// File path or external URL.
    pub target: String,
    /// Optional line number for file search matches.
    pub line: Option<u32>,
    /// Snippet or page title.
    pub snippet: String,
    /// True when `target` is an external web URL.
    pub is_url: bool,
}

/// A rich tool call row projected from wire [`viz_client::protocol::ToolCall`].
#[derive(Debug, Clone)]
pub struct Tool {
    /// Stable element key.
    pub key: String,
    /// The call's id, pairing it with its result.
    pub call_id: String,
    /// Committed entry id when persisted, used for `ToolUpdate` and chunk
    /// loading.
    pub entry: Option<EntryId>,
    /// Live segment key when uncommitted, used for live chunk loading.
    pub live_key: Option<String>,
    /// Normalized category/tool name.
    pub name: String,
    /// Original programmatic tool name, when reported.
    pub raw_name: Option<String>,
    /// Display title from the harness.
    pub title: String,
    /// One-line summary of the arguments.
    pub summary: String,
    /// Recognized shell command text, never a serialized arguments object.
    pub input: Option<String>,
    /// The original arguments while the harness may still be waiting for
    /// approval.
    pub pending_input: Option<String>,
    /// Pretty-printed structured arguments for MCP/dynamic tool inspectors.
    pub raw_input_pretty: Option<String>,
    /// Command timeout in milliseconds, when advertised.
    pub timeout_ms: Option<u64>,
    /// Unix milliseconds when the tool started running.
    pub started_at: Option<i64>,
    /// Unix milliseconds when the tool finished.
    pub ended_at: Option<i64>,
    /// Progress.
    pub status: ToolStatus,
    /// Exit status code reported by the harness or terminal envelope.
    pub exit_code: Option<i32>,
    /// Termination signal name, when killed by a signal.
    pub exit_signal: Option<String>,
    /// Display-ready result text or running output.
    pub output: String,
    /// Associated terminal id for jump-to-terminal hooks.
    pub terminal_id: Option<String>,
    /// True when the beginning of terminal output was dropped.
    pub terminal_truncated: bool,
    /// True when full payload can be fetched lazily via `thread.entry_chunk`.
    pub more: bool,
    /// Automatic approval review verdict (e.g. Guardian Review).
    pub review: Option<Review>,
    /// The task confirmed by a viz delegation receipt.
    pub task_id: Option<String>,
    /// The thread a subagent call delegated to.
    pub child: Option<ThreadId>,
    /// Target environment for the delegated child thread, if remote.
    pub child_environment: Option<String>,
    /// The viz tool an ACP call ran.
    pub viz: Option<String>,
    /// File diffs reported by the tool call.
    pub diffs: Vec<ToolDiff>,
    /// Structured file-search or web-search matches parsed from output.
    pub search_matches: Vec<SearchMatch>,
}

/// One checklist step in an ACP plan entry.
#[derive(Debug, Clone)]
pub struct PlanStep {
    /// Step description.
    pub text: String,
    /// Priority advertised by the harness.
    pub priority: PlanPriority,
    /// Step completion status.
    pub status: ToolStatus,
}

impl PlanCard {
    pub(super) fn from_wire(key: String, plan: &Plan) -> Self {
        let content = match &plan.content {
            PlanContent::Items { entries } => PlanBody::Items(
                entries
                    .iter()
                    .map(|step| PlanStep {
                        text: step.content.clone(),
                        priority: step.priority,
                        status: match step.status {
                            PlanStatus::Completed => ToolStatus::Done,
                            PlanStatus::InProgress => ToolStatus::Running,
                            PlanStatus::Pending => ToolStatus::Pending,
                        },
                    })
                    .collect(),
            ),
            PlanContent::Markdown { text, path } => PlanBody::Markdown {
                text: text.clone(),
                path: path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            },
        };
        Self {
            key,
            at: plan.at,
            content,
        }
    }
}

impl CompactionNotice {
    pub(super) fn from_wire(key: String, c: &Compaction) -> Self {
        Self {
            key,
            id: c.id.clone(),
            status: c.status,
            summary: c.summary.clone(),
            error: c.error.clone(),
            started_at: c.started_at,
            ended_at: c.ended_at,
        }
    }
}

impl Item {
    pub(super) fn from_segment(
        key: String,
        entry_id: Option<EntryId>,
        segment: Segment,
        more: bool,
        thinking: bool,
    ) -> Self {
        if thinking {
            Self::Thinking {
                key,
                entry_id,
                text: segment.text,
                started_at: segment.started_at,
                ended_at: segment.ended_at,
                more,
            }
        } else {
            Self::Text {
                key,
                entry_id,
                text: segment.text,
                started_at: segment.started_at,
                ended_at: segment.ended_at,
                more,
            }
        }
    }
}
