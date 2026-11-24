//! What agents see of the workspace: the facts they act on, within a bound.
//! Clients get catalogs, bodies and raw output over the protocol instead.

use std::fmt::Write as _;

use serde::Serialize;
use viz_protocol::{
    AccessKind, Attention, Body, Entry, EntryData, EntryId, EntryPage, OptionValue, PlanContent,
    Run, RunId, RunRequest, Shelf, Task, TaskId, TaskStatus, ThreadId, ThreadState, ThreadStatus,
    ThreadSummary, ToolCall, ToolCallStatus, WaitResult,
};

/// Characters of a message, and of a tool's output, shown in a page.
const TEXT: usize = 2_000;
const OUTPUT: usize = 600;
const PREVIEW: usize = 200;
/// Characters of a result or of one entry read whole.
const WHOLE: usize = 30_000;

/// The start and end of a text that is too long.
fn bounded(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let head: String = text.chars().take(limit * 3 / 4).collect();
    let tail: String = text.chars().skip(count - limit / 4).collect();
    let omitted = count - limit * 3 / 4 - limit / 4;
    format!("{head}\n[… {omitted} characters omitted …]\n{tail}")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Thread {
    id: ThreadId,
    title: String,
    status: ThreadStatus,
    shelf: Shelf,
    #[serde(skip_serializing_if = "Option::is_none")]
    attention: Option<Attention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<ThreadId>,
    harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    updated_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Threads {
    threads: Vec<Thread>,
    /// How many more match.
    omitted: usize,
}

pub(super) fn threads(threads: Vec<ThreadSummary>, limit: usize) -> Threads {
    let omitted = threads.len().saturating_sub(limit);
    Threads {
        threads: threads
            .into_iter()
            .take(limit)
            .map(|thread| Thread {
                id: thread.id,
                title: thread.title,
                status: thread.status,
                shelf: thread.shelf,
                attention: thread.attention,
                project: thread.project,
                worktree: thread.worktree.map(|worktree| worktree.path),
                parent: thread.parent,
                harness: thread.harness,
                model: thread.model,
                updated_at: thread.updated_at,
            })
            .collect(),
        omitted,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct State {
    status: ThreadStatus,
    harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    /// The selected value of each option.
    options: Vec<Selected>,
    queue: Vec<Queued>,
    /// The end of what the agent is saying right now.
    #[serde(skip_serializing_if = "Option::is_none")]
    live: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    running_tools: Vec<String>,
    access: AccessKind,
    /// Approvals and questions waiting for an answer; answer with
    /// `thread_answer`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    requests: Vec<RunRequest>,
}

#[derive(Debug, Serialize)]
struct Selected {
    id: String,
    value: OptionValue,
}

#[derive(Debug, Serialize)]
struct Queued {
    run: RunId,
    text: String,
}

pub(super) fn state(state: ThreadState) -> State {
    let (live, running_tools) = state.live.map_or((None, Vec::new()), |live| {
        let mut prose = None;
        let mut tools = Vec::new();
        for entry in live.entries {
            match entry.body {
                Body::Text(segment) | Body::Thought(segment) => {
                    let skip = segment.text.chars().count().saturating_sub(OUTPUT);
                    prose = Some(segment.text.chars().skip(skip).collect());
                }
                Body::Tool(tool) => tools.push(bounded(&tool.title, PREVIEW)),
                _ => {}
            }
        }
        (prose, tools)
    });
    State {
        status: state.status,
        harness: state.harness,
        project: state.project,
        cwd: state.cwd,
        worktree: state.worktree.map(|worktree| worktree.path),
        options: state
            .options
            .into_iter()
            .map(|option| Selected {
                id: option.id,
                value: option.value,
            })
            .collect(),
        queue: state
            .queue
            .into_iter()
            .map(|input| Queued {
                run: input.run,
                text: bounded(&input.prompt.display_text(), PREVIEW),
            })
            .collect(),
        live,
        running_tools,
        access: state.access.current.kind,
        requests: state.requests,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Page {
    entries: Vec<Line>,
    /// Pass it to read what came earlier.
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<EntryId>,
}

#[derive(Debug, Serialize)]
struct Line {
    id: EntryId,
    kind: &'static str,
    text: String,
}

pub(super) fn page(page: EntryPage) -> Page {
    Page {
        entries: page
            .entries
            .into_iter()
            .map(|entry| line(entry, TEXT, OUTPUT))
            .collect(),
        before: page.before,
    }
}

/// One entry with as much of it as an agent can take.
pub(super) fn whole(entry: Entry) -> Page {
    Page {
        entries: vec![line(entry, WHOLE, WHOLE)],
        before: None,
    }
}

fn line(entry: Entry, text: usize, output: usize) -> Line {
    let (kind, text) = match entry.data {
        EntryData::Prompt { prompt, .. } => {
            let mut shown = bounded(&prompt.display_text(), text);
            for anchor in prompt.anchors() {
                if let viz_protocol::PromptRef::Attachment { name, .. } = anchor.target {
                    let _ = write!(shown, "\n[attachment {name}]");
                }
            }
            ("prompt", shown)
        }
        EntryData::Transcript(entry) => match entry.body {
            Body::Text(said) => ("text", bounded(&said.text, text)),
            Body::Thought(thought) => ("thought", bounded(&thought.text, output)),
            Body::Tool(record) => ("tool", tool(&record, output)),
            Body::Plan(plan) => {
                let text = match plan.content {
                    PlanContent::Items { entries } => entries
                        .iter()
                        .map(|entry| entry.content.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    PlanContent::Markdown { text, .. } => text,
                };
                ("plan", bounded(&text, output))
            }
            Body::Media(media) => ("media", format!("{:?}", media.block.kind)),
            Body::Compaction(compaction) => (
                "compaction",
                bounded(
                    compaction.error.as_deref().unwrap_or(&compaction.summary),
                    output,
                ),
            ),
        },
        EntryData::ToolUpdate {
            entry: updated,
            record,
        } => (
            "tool_update",
            format!("[updates {updated}] {}", tool(&record, output)),
        ),
        EntryData::Run(end) => (
            "run_end",
            end.error
                .or(end.stop_reason)
                .unwrap_or_else(|| "ended".into()),
        ),
        EntryData::Continued => ("continued", String::new()),
        EntryData::TaskResult {
            text: result, task, ..
        } => (
            "task_result",
            format!("[{task}] {}", bounded(&result, text)),
        ),
    };
    Line {
        id: entry.id,
        kind,
        text,
    }
}

fn tool(record: &ToolCall, output: usize) -> String {
    let status = match record.status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "running",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
    };
    let mut text = format!("{} ({status})", bounded(&record.title, PREVIEW));
    for diff in record.diffs().take(12) {
        let _ = write!(text, "\nchanged {}", diff.path);
    }
    if let Some(shown) = record.output_text() {
        text.push('\n');
        text.push_str(&bounded(&shown, output));
    }
    text
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Waited {
    run: Option<Run>,
    timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'static str>,
}

const AGAIN: &str = "still running; call again to keep waiting";

pub(super) fn waited(result: WaitResult) -> Waited {
    Waited {
        run: result.run.map(|run| Run {
            answer: run.answer.map(|answer| bounded(&answer, WHOLE)),
            ..run
        }),
        timed_out: result.timed_out,
        hint: result.timed_out.then_some(AGAIN),
    }
}

#[derive(Debug, Serialize)]
pub(super) struct Tasks {
    tasks: Vec<TaskLine>,
}

#[derive(Debug, Serialize)]
struct TaskLine {
    id: String,
    title: String,
    status: TaskStatus,
    environment: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unreachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread: Option<ThreadId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub(super) fn tasks(tasks: Vec<Task>) -> Tasks {
    Tasks {
        tasks: tasks
            .into_iter()
            .map(|task| TaskLine {
                id: task.id,
                title: task.title,
                status: task.status,
                environment: task.environment,
                unreachable: !task.reachable,
                thread: task.thread,
                error: task.error.map(|error| bounded(&error, PREVIEW)),
            })
            .collect(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskView {
    task: Task,
    timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'static str>,
}

pub(super) fn task(task: Task, timed_out: bool) -> TaskView {
    TaskView {
        task: Task {
            result: task.result.map(|result| bounded(&result, WHOLE)),
            ..task
        },
        timed_out,
        hint: timed_out.then_some(AGAIN),
    }
}

#[derive(Debug, Serialize)]
pub(super) struct Deleted {
    pub(super) deleted: TaskId,
}
