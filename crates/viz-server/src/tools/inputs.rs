//! MCP input projections for typed workspace methods. Some tools expose a
//! subset of the underlying request, such as sends without connector origin.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use viz_core::threads::Patch;
use viz_protocol::{
    Empty, LaunchThread, ListEntries, MoveThread, ReadThreadState, SendInput, SendMode, StopThread,
    ThreadId, WaitThread,
};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unread: Option<bool>,
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
    StopThread => |input: &StopThread| input.thread,
    MoveThread => |input: &MoveThread| input.thread,
    SendText => |input: &SendText| input.thread,
    ThreadUpdate => |input: &ThreadUpdate| input.thread,
}

impl ToolInput for WaitThread {
    fn thread(&self) -> Option<ThreadId> {
        Some(self.thread)
    }

    fn extra_wait(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(30_000).min(120_000))
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
            title: input.title,
            archived: input.archived,
            pinned: input.pinned,
            unread: input.unread,
            project: Patch::Keep,
        }
    }
}
