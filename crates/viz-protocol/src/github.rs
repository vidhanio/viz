//! GitHub context and actions for a registered project or a thread checkout.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ThreadId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubAuth {
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHubAuthState {
    Ready,
    GhMissing,
    LoggedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHubItemKind {
    Issue,
    PullRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubSearch {
    pub project: String,
    pub kind: GitHubItemKind,
    pub query: String,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubSearchResult {
    pub items: Vec<GitHubItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubItem {
    pub kind: GitHubItemKind,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubPull {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: GitHubPullState,
    pub draft: bool,
    pub head: String,
    pub head_commit: String,
    pub base: String,
    pub additions: u64,
    pub deletions: u64,
    pub mergeable: String,
    pub checks: Option<String>,
    pub review: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHubPullState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubCreatePull {
    pub thread: ThreadId,
    pub base: Option<String>,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubPullPage {
    pub thread: ThreadId,
    pub number: u64,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubChecks {
    pub thread: ThreadId,
    pub commit: String,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubCheck {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubReview {
    pub id: u64,
    pub author: Option<String>,
    pub state: String,
    pub body: Option<String>,
    pub url: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHubLaunch {
    pub number: u64,
    pub launch: crate::LaunchThread,
}
