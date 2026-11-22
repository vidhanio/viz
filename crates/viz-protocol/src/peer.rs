//! Server-to-server methods for durable delegation. These procedures require
//! peer authentication and are excluded from the client method schema.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Attachment, Method, Run, RunId, ThreadId, WorktreeTarget, private};

/// A method restricted to configured peers.
pub trait PeerMethod: Method + DeserializeOwned + Send {
    /// Methods carrying an environment ID bind it to the authenticated sender.
    fn source(&self) -> Option<&str> {
        None
    }
}

/// An offer may be retried with the same environment and task key.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveTask {
    pub task: String,
    pub origin_environment: String,
    pub parent: NonZeroU64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<NonZeroU64>,
    pub text: String,
    #[serde(flatten)]
    pub target: TaskTarget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

/// Placement and initial runner options for an offered task.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

/// The durable recipient thread and run allocated for an offer.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Received {
    pub thread: NonZeroU64,
    pub run: NonZeroU64,
}

impl PeerMethod for ReceiveTask {
    fn source(&self) -> Option<&str> {
        Some(&self.origin_environment)
    }
}

/// Retract an offer, or return its receipt when it has already been admitted.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetractTask {
    pub origin_environment: String,
    pub task: String,
}

impl PeerMethod for RetractTask {
    fn source(&self) -> Option<&str> {
        Some(&self.origin_environment)
    }
}

/// Query the assigned run and optionally durably stop its family.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressRequest {
    pub thread: ThreadId,
    pub run: RunId,
    #[serde(default)]
    pub cancel: bool,
}

impl PeerMethod for ProgressRequest {}

/// The aggregate outcome of the root and its descendant wakes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressResult {
    #[serde(deserialize_with = "required_option")]
    pub run: Option<Run>,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

/// A settled continuation identified by its child environment and run.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUp {
    pub source: String,
    pub task: String,
    pub thread: NonZeroU64,
    pub run: NonZeroU64,
    pub text: String,
}

impl PeerMethod for FollowUp {
    fn source(&self) -> Option<&str> {
        Some(&self.source)
    }
}

macro_rules! peer_methods {
    ($($params:ty => $name:literal, $response:ty;)*) => {
        $(
            impl private::Sealed for $params {}
            impl Method for $params {
                const NAME: &'static str = $name;
                type Response = $response;
            }
        )*
    };
}

peer_methods! {
    ReceiveTask => "task.receive", Received;
    RetractTask => "task.retract", Option<Received>;
    ProgressRequest => "task.progress", ProgressResult;
    FollowUp => "task.follow_up", bool;
}
