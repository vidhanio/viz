//! Subscriptions, environment discovery, run outcomes, and delegated tasks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    Attachment, Empty, Entry, EntryId, Origin, RunId, ThreadId, ThreadState, WorktreeTarget,
};

/// `thread.watch` params: start receiving
/// [`Notification::ThreadView`] for a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Watch {
    /// The thread.
    pub thread: ThreadId,
}

/// `thread.unwatch` params: stop watching a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unwatch {
    /// The thread.
    pub thread: ThreadId,
}

/// `thread.watch` and `thread.unwatch` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Watching {
    /// Whether this client now watches the thread.
    pub watching: bool,
}

/// Notifications pushed after hello.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "method", content = "params")]
pub enum Notification {
    #[serde(rename = "reply")]
    Reply {
        origin: Origin,
        thread: ThreadId,
        run: RunId,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "thread.view")]
    ThreadView {
        thread: ThreadId,
        entries: Vec<Entry>,
        reset: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<EntryId>,
    },
    #[serde(rename = "thread.state")]
    ThreadState {
        thread: ThreadId,
        state: ThreadState,
    },
    #[serde(rename = "threads.changed")]
    ThreadsChanged(Empty),
    #[serde(rename = "tasks.changed")]
    TasksChanged(Empty),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ListEnvironments {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct EnvironmentList {
    pub environments: Vec<Environment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Environment {
    pub id: String,
    pub url: Option<String>,
    pub status: EnvironmentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentStatus {
    Local,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitThread {
    pub thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WaitResult {
    pub run: Option<Run>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Run {
    pub id: RunId,
    pub thread: ThreadId,
    /// What started the run.
    pub kind: RunKind,
    pub status: RunStatus,
    pub answer: Option<String>,
    pub error: Option<String>,
}

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    /// An input: a prompt entry starts it and the ACP prompt response ends it.
    Prompt,
    /// The agent continued on its own after its turn ended, such as when a
    /// backgrounded command finished. A `continued` entry starts it; the
    /// server ends it when the agent goes quiet or an input is waiting.
    Unprompted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadThreadState {
    pub thread: ThreadId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct ThreadStateResult {
    pub state: ThreadState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ReadProject<'a> {
    pub project: &'a str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListTasks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct TaskList {
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Task {
    pub id: String,
    pub parent: ThreadId,
    pub environment: String,
    pub thread: Option<ThreadId>,
    pub run: Option<RunId>,
    pub status: TaskStatus,
    pub reachable: bool,
    pub title: String,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusRequest<'a> {
    pub task: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusResult {
    pub task: Task,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CancelTask<'a> {
    pub task: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct TaskResult {
    pub task: Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DelegateTask<'a> {
    pub parent: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<&'a WorktreeTarget>,
    pub text: &'a str,
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub attachments: &'a [Attachment],
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub attachment_ids: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<&'a str>,
}

const fn slice_is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}
