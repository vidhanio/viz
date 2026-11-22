//! Typed GitHub responses used by the workspace.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Repository {
    pub name: String,
    pub full_name: String,
    pub default_branch: String,
    pub clone_url: String,
    pub ssh_url: String,
    pub html_url: String,
    pub owner: Account,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    #[serde(rename = "isDraft")]
    pub draft: bool,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub base_ref_name: String,
    pub base_ref_oid: String,
    pub additions: u64,
    pub deletions: u64,
    pub author: Option<PullRequestAuthor>,
    pub updated_at: String,
    pub mergeable: String,
    pub review_decision: Option<String>,
    #[serde(skip)]
    pub checks_rollup: Option<String>,
}

/// GitHub actor, which may be a user, bot, or organization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestAuthor {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Issue,
    PullRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullComment {
    pub id: u64,
    pub body: String,
    pub html_url: String,
    pub user: Option<Account>,
    pub created_at: String,
    /// Present on inline review comments.
    pub path: Option<String>,
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PullReview {
    pub id: u64,
    pub body: Option<String>,
    pub state: String,
    pub user: Option<Account>,
    pub html_url: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestMode {
    Ready,
    Draft,
}

/// Input for a same-repository pull request. The default branch is used when
/// base is absent.
#[derive(Debug, Clone, Copy)]
pub struct CreatePullRequest<'a> {
    pub head: &'a str,
    pub base: Option<&'a str>,
    pub title: &'a str,
    pub body: &'a str,
    pub mode: PullRequestMode,
}

/// A template path on the repository's default branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub path: String,
}
