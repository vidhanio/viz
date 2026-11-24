//! Subscriptions, environment discovery, run outcomes, and delegated tasks.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    BlobAttachment, Empty, Entry, EntryId, Origin, RunId, TaskId, ThreadId, ThreadState,
    WorktreeTarget,
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
        state: Box<ThreadState>,
    },
    #[serde(rename = "git.changed")]
    GitChanged {
        thread: ThreadId,
        status: Option<crate::GitStatus>,
    },
    #[serde(rename = "thread.live")]
    ThreadLive {
        thread: ThreadId,
        delta: crate::LiveDelta,
    },
    #[serde(rename = "threads.changed")]
    ThreadsChanged(Empty),
    #[serde(rename = "tasks.changed")]
    TasksChanged(Empty),
    /// A harness's status or options changed; list harnesses again.
    #[serde(rename = "harnesses.changed")]
    HarnessesChanged(Empty),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::TurnUsage>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadProject {
    pub project: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTasks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskStatusRequest {
    #[schemars(length(min = 1))]
    pub task: TaskId,
    #[serde(default, skip_serializing_if = "is_false")]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 120_000))]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusResult {
    pub task: Task,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelTask {
    #[schemars(length(min = 1))]
    pub task: TaskId,
}

/// Remove a task from its parent's list once it is over and its result
/// delivered. Its thread stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteTask {
    #[schemars(length(min = 1))]
    pub task: TaskId,
}

/// Remove every task of a thread that is over and whose result was delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClearTasks {
    pub parent: ThreadId,
}

/// `tasks.clear` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cleared {
    /// How many tasks were removed.
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct TaskResult {
    pub task: Task,
}

/// Parameters for delegating a task from a parent thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegateInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Checkout of the project to run in: `{"mode": "new"}` isolates the
    /// child on a new branch. Omit to use `project`'s checkout, or the
    /// parent's checkout when `project` is omitted too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    #[schemars(length(min = 1))]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<BlobAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 256))]
    pub request_id: Option<String>,
    /// A short name for the task and its new thread. Defaults to the start of
    /// `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 200))]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DelegateTask {
    pub parent: ThreadId,
    #[serde(flatten)]
    pub input: DelegateInput,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}
