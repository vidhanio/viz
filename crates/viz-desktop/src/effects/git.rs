//! Typed Git and GitHub requests on the active session.

use viz_client::{
    Error,
    protocol::{
        BranchList, CheckpointEdge, GitAction, GitActionRequest, GitDiff, GitDiffRequest,
        GitDiffTarget, GitFailure, GitHubCheck, GitHubChecks, GitHubCreatePull, GitHubPullPage,
        GitHubReview, InitRepository, ListBranches, RestoreCheckpoint, RunId, ThreadId, UnwatchGit,
        WatchGit,
    },
};

use super::server::Session;
use crate::update::{GitMsg, ServerMsg};

#[derive(Debug, Clone)]
pub enum Request {
    Watch(Option<ThreadId>),
    Branches {
        project: String,
        query: String,
        offset: Option<usize>,
    },
    Diff(GitDiffTarget),
    Preview,
    Action(GitAction),
    CreateCheckout {
        name: String,
        start: String,
    },
    Init(String),
    Checks(String),
    Reviews(u64),
    CreatePull {
        title: String,
        body: String,
        draft: bool,
        base: Option<String>,
    },
    Restore {
        run: RunId,
        expected_run: RunId,
    },
}

#[derive(Debug, Clone)]
pub enum Reply {
    Branches { page: BranchList, append: bool },
    Diff(GitDiff),
    Preview(GitDiff),
    Checks(Vec<GitHubCheck>),
    Reviews(Vec<GitHubReview>),
    Done,
}

impl Session<'_> {
    pub(super) async fn git(
        &mut self,
        thread: ThreadId,
        serial: u64,
        request: Request,
    ) -> viz_client::Result<()> {
        if let Request::Watch(next) = request {
            if let Some(previous) = self.git_watched.take() {
                self.client
                    .request(&UnwatchGit { thread: previous })
                    .await?;
            }
            if let Some(thread) = next.filter(|id| self.watched == Some(*id)) {
                match self.client.request(&WatchGit { thread }).await {
                    Ok(status) => {
                        self.git_watched = Some(thread);
                        self.dispatch(ServerMsg::Git(GitMsg::Status(thread, Some(status))));
                    }
                    Err(error) => return Err(error),
                }
            }
            return Ok(());
        }
        if self.watched != Some(thread) {
            return Ok(());
        }
        let result = self.git_request(thread, request).await;
        if matches!(result, Err(Error::Closed)) {
            return Err(Error::Closed);
        }
        let result = result.map_err(|error| {
            let hint = match &error {
                Error::Rpc {
                    git_failure: Some(GitFailure::Busy),
                    ..
                } => "The checkout is busy.",
                Error::Rpc {
                    git_failure: Some(GitFailure::Conflicted),
                    ..
                } => "Resolve checkout conflicts first.",
                Error::Rpc {
                    git_failure: Some(GitFailure::HookRejected),
                    ..
                } => "A Git hook rejected this change.",
                Error::Rpc {
                    git_failure: Some(GitFailure::Authentication),
                    ..
                } => "Git authentication failed.",
                _ => return error.to_string(),
            };
            format!("{hint} {error}")
        });
        self.dispatch(ServerMsg::Git(GitMsg::Reply {
            thread,
            serial,
            result,
        }));
        Ok(())
    }

    async fn git_request(&self, thread: ThreadId, request: Request) -> viz_client::Result<Reply> {
        let client = &self.client;
        Ok(match request {
            Request::Watch(_) => unreachable!("watch is handled by the session"),
            Request::Branches {
                project,
                query,
                offset,
            } => Reply::Branches {
                page: client
                    .request(&ListBranches {
                        project: &project,
                        query: Some(&query),
                        offset,
                        limit: Some(50),
                    })
                    .await?,
                append: offset.is_some(),
            },
            Request::Diff(target) => {
                Reply::Diff(client.request(&GitDiffRequest { thread, target }).await?)
            }
            Request::Preview => Reply::Preview(
                client
                    .request(&GitDiffRequest {
                        thread,
                        target: GitDiffTarget::Working,
                    })
                    .await?,
            ),
            Request::Checks(commit) => Reply::Checks(
                client
                    .request(&GitHubChecks {
                        thread,
                        commit,
                        page: None,
                    })
                    .await?,
            ),
            Request::Reviews(number) => Reply::Reviews(
                client
                    .request(&GitHubPullPage {
                        thread,
                        number,
                        page: None,
                    })
                    .await?,
            ),
            Request::Action(action) => {
                client.request(&GitActionRequest { thread, action }).await?;
                Reply::Done
            }
            Request::CreateCheckout { name, start } => {
                self.create_checkout(thread, name, start).await?
            }
            Request::Init(path) => {
                client.request(&InitRepository { path }).await?;
                self.refresh_list().await?;
                Reply::Done
            }
            Request::CreatePull {
                title,
                body,
                draft,
                base,
            } => {
                client
                    .request(&GitHubCreatePull {
                        thread,
                        base,
                        title,
                        body,
                        draft,
                    })
                    .await?;
                self.refresh_list().await?;
                Reply::Done
            }
            Request::Restore { run, expected_run } => {
                client
                    .request(&RestoreCheckpoint {
                        thread,
                        run,
                        expected_run,
                        edge: CheckpointEdge::Before,
                    })
                    .await?;
                Reply::Done
            }
        })
    }

    async fn create_checkout(
        &self,
        thread: ThreadId,
        name: String,
        start: String,
    ) -> viz_client::Result<Reply> {
        self.client
            .request(&GitActionRequest {
                thread,
                action: GitAction::CreateBranch {
                    name: name.clone(),
                    start,
                },
            })
            .await?;
        self.client
            .request(&GitActionRequest {
                thread,
                action: GitAction::SwitchBranch { name },
            })
            .await?;
        Ok(Reply::Done)
    }
}
