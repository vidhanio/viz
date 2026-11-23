//! Pure derivations for delegated tasks and subagent tool calls.

use viz_client::protocol::{Task, TaskStatus, ThreadId, ThreadStatus, ThreadSummary};

use super::{Model, Tool, ToolStatus};

/// How a viz tool call relates to a delegated subagent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentCall {
    /// Spawned a new delegated task or thread.
    Spawn,
    /// Sent input to an existing thread.
    Message,
    /// Checked status or read transcript.
    Read,
    /// Listed tasks.
    List,
    /// Cancelled a task or stopped a thread.
    Cancel,
}

/// Where a delegated subagent stands: its live task/thread status, else its
/// tool call status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStanding {
    /// Actively working.
    Working,
    /// Queued waiting to run.
    Queued,
    /// Completed successfully.
    Done,
    /// Failed with error.
    Failed,
    /// Stopped / cancelled.
    Stopped,
    /// Peer environment unreachable.
    Unreachable,
}

impl AgentStanding {
    /// Maps task lifecycle and connectivity to the displayed status.
    #[must_use]
    pub const fn from_task(task: &Task) -> Self {
        if !task.reachable {
            return Self::Unreachable;
        }
        match task.status {
            TaskStatus::Running => Self::Working,
            TaskStatus::Queued => Self::Queued,
            TaskStatus::Completed => Self::Done,
            TaskStatus::Failed => Self::Failed,
            TaskStatus::Cancelled => Self::Stopped,
        }
    }

    /// Derives the standing of a subagent call from its task or tool state.
    #[must_use]
    pub fn of(model: &Model, tool: &Tool, task: Option<&Task>) -> Self {
        if let Some(task) = task {
            return Self::from_task(task);
        }
        let status = local_child(model, tool, None).map(|thread| thread.status);
        match (status, tool.status) {
            (Some(ThreadStatus::Working), _) | (None, ToolStatus::Running) => Self::Working,
            (Some(ThreadStatus::Failed), _) | (None, ToolStatus::Failed) => Self::Failed,
            (None, ToolStatus::Abandoned) => Self::Stopped,
            (None, ToolStatus::Pending) => Self::Queued,
            (None, ToolStatus::Done) if tool.child.is_none() => Self::Queued,
            _ => Self::Done,
        }
    }

    /// Capitalized status word for a single subagent card.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Working => "Running",
            Self::Queued => "Queued",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::Unreachable => "Unreachable",
        }
    }
}

/// Classifies a tool call if it targets a viz subagent / delegation tool.
#[must_use]
pub fn subagent_call(tool: &Tool) -> Option<SubagentCall> {
    let name = tool.viz.as_deref().unwrap_or(tool.name.as_str());
    match name {
        "delegate_task" => Some(SubagentCall::Spawn),
        "thread_launch" if tool.child.is_some() => Some(SubagentCall::Spawn),
        "thread_send" => Some(SubagentCall::Message),
        "task_status" | "thread_read" => Some(SubagentCall::Read),
        "task_list" => Some(SubagentCall::List),
        "task_cancel" | "thread_stop" => Some(SubagentCall::Cancel),
        _ => None,
    }
}

/// Whether `tool` spawned a subagent and did not fail before creation.
#[must_use]
pub fn spawned_subagent(tool: &Tool) -> bool {
    subagent_call(tool) == Some(SubagentCall::Spawn) && tool.status != ToolStatus::Failed
}

/// Resolves the target thread and optional remote environment of a delegation.
#[must_use]
pub fn child_link<'a>(
    tool: &'a Tool,
    task: Option<&'a Task>,
) -> Option<(ThreadId, Option<&'a str>)> {
    task.and_then(|task| {
        task.thread
            .map(|child| (child, Some(task.environment.as_str())))
    })
    .or_else(|| {
        tool.child
            .map(|child| (child, tool.child_environment.as_deref()))
    })
}

/// Finds the cached local child thread summary for a delegation, if local.
#[must_use]
pub fn local_child<'a>(
    model: &'a Model,
    tool: &Tool,
    task: Option<&Task>,
) -> Option<&'a ThreadSummary> {
    let (child, environment) = child_link(tool, task)?;
    environment
        .is_none_or(|environment| model.connection.environment.as_deref() == Some(environment))
        .then(|| model.summary(child))
        .flatten()
}

/// Matches a tool call to its durable task record under the selected thread.
#[must_use]
pub fn delegated_task<'a>(model: &'a Model, tool: &Tool) -> Option<&'a Task> {
    if let Some(id) = &tool.task_id {
        return model
            .tasks()
            .iter()
            .find(|task| task.id == *id && Some(task.parent) == model.selection.selected);
    }
    let child = tool.child?;
    let environment = tool
        .child_environment
        .as_deref()
        .or(model.connection.environment.as_deref());
    model.tasks().iter().find(|task| {
        Some(task.parent) == model.selection.selected
            && task.thread == Some(child)
            && Some(task.environment.as_str()) == environment
    })
}

/// Prefers the child thread's title over the task title or call summary.
#[must_use]
pub fn subagent_title<'a>(
    tool: &'a Tool,
    task: Option<&'a Task>,
    child: Option<&'a ThreadSummary>,
) -> &'a str {
    child
        .map(|summary| summary.title.as_str())
        .filter(|title| !title.is_empty())
        .or_else(|| task.map(|task| task.title.as_str()))
        .unwrap_or(&tool.summary)
}
