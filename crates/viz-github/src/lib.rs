//! Host-scoped GitHub operations for the workspace runtime.
//!
//! The client resolves credentials on demand through gh and caches successful
//! reads. Create calls are explicit and never retried. A client is bound to one
//! host so a repository remote cannot redirect its credentials.

mod client;
mod models;
mod repository;

pub use client::{AuthState, Error, GitHub, PollState, TransportFailure, poll_delay};
pub use models::{
    Account, CheckRun, CreatePullRequest, Issue, IssueDetail, IssueKind, PullComment, PullRequest,
    PullRequestAuthor, PullRequestMode, PullReview, Repository, Template,
};
pub use viz_git::RemoteRepository;
