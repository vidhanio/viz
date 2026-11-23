//! MCP input projections for typed workspace methods. Some tools expose a
//! subset of the underlying request, such as sends without connector origin.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use viz_core::threads::{Changes, Patch};
use viz_protocol::{
    Empty, ForkThread, LaunchThread, ListEntries, MoveThread, ReadThreadState, RetitleThread,
    RunId, SendInput, SendMode, StopThread, ThreadId, TriageThread, WaitThread,
};

use super::{DEFAULT_WAIT, MAX_WAIT};
use crate::workspace::rpc::{
    ListThreads as ThreadList, ProjectCreate, ProjectKey, ProjectUpdate, Search, UpdateMetadata,
    WorktreeRemove,
};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct SendText {
    thread: ThreadId,
    #[schemars(length(min = 1))]
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<SendMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub(super) struct ThreadUpdate {
    thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// Tool inputs can project thread context and timeouts without inspecting JSON.
pub(super) trait ToolInput: DeserializeOwned + Serialize + Send + Sync + 'static {
    fn thread(&self) -> Option<ThreadId> {
        None
    }

    fn extra_wait(&self) -> Duration {
        Duration::ZERO
    }
}

macro_rules! tool_inputs {
    ($($input:ty),+ $(,)?) => { $(impl ToolInput for $input {})+ };
}

tool_inputs!(
    Empty,
    ThreadList,
    Search,
    LaunchThread,
    ProjectKey,
    ProjectCreate,
    ProjectUpdate,
    WorktreeRemove
);

macro_rules! thread_inputs {
    ($($input:ty => $thread:expr),+ $(,)?) => {
        $(impl ToolInput for $input {
            fn thread(&self) -> Option<ThreadId> { Some(($thread)(self)) }
        })+
    };
}

thread_inputs! {
    ListEntries => |input: &ListEntries| input.thread,
    ReadThreadState => |input: &ReadThreadState| input.thread,
    ForkThread => |input: &ForkThread| input.thread,
    StopThread => |input: &StopThread| input.thread,
    MoveThread => |input: &MoveThread| input.thread,
    SendText => |input: &SendText| input.thread,
    ThreadUpdate => |input: &ThreadUpdate| input.thread,
    TriageThread => |input: &TriageThread| input.thread,
    RetitleThread => |input: &RetitleThread| input.thread,
}

/// A wait short enough for an MCP client.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct WaitRun {
    thread: ThreadId,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<RunId>,
    /// 30 seconds by default; longer waits are cut to 45 seconds.
    #[serde(default = "default_wait", deserialize_with = "capped_wait")]
    #[schemars(range(max = 45_000))]
    timeout_ms: u64,
}

const fn default_wait() -> u64 {
    DEFAULT_WAIT
}

fn capped_wait<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let wait = Option::<u64>::deserialize(deserializer)?;
    Ok(wait.unwrap_or(DEFAULT_WAIT).min(MAX_WAIT))
}

impl ToolInput for WaitRun {
    fn thread(&self) -> Option<ThreadId> {
        Some(self.thread)
    }

    fn extra_wait(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

impl From<WaitRun> for WaitThread {
    fn from(input: WaitRun) -> Self {
        Self {
            thread: input.thread,
            run: input.run,
            timeout_ms: Some(input.timeout_ms),
        }
    }
}

impl From<SendText> for SendInput {
    fn from(input: SendText) -> Self {
        Self {
            thread: input.thread,
            text: input.text,
            mode: input.mode,
            request_id: input.request_id,
            origin: None,
            attachments: Vec::new(),
        }
    }
}

impl From<ThreadUpdate> for UpdateMetadata {
    fn from(input: ThreadUpdate) -> Self {
        Self {
            thread: input.thread,
            changes: Changes {
                title: input.title,
                project: Patch::Keep,
            },
        }
    }
}
