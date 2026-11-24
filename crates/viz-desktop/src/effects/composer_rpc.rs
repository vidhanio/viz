//! Asynchronous composer RPC helpers: ranked file search, GitHub `#`
//! pull-request/issue search, queued-input mutations.

use viz_client::{
    Result,
    protocol::{
        GitHubItemKind, GitHubSearch, Patch, RemoveQueuedInput, RunId, SearchFiles,
        SteerQueuedInput, ThreadId, UpdateQueuedInput,
    },
};

use super::server::Session;
use crate::update::ServerMsg;

impl Session<'_> {
    /// Searches a checkout or registered project for files and directories,
    /// returning ranked [`viz_client::protocol::FileSearchMatch`] entries.
    pub(super) fn search_files(
        &mut self,
        thread: Option<ThreadId>,
        project: Option<String>,
        request: u64,
        query: String,
    ) {
        let client = self.client.clone();
        self.spawn_msg(async move {
            let result = client
                .search_files(&SearchFiles {
                    thread,
                    project,
                    query,
                    limit: Some(40),
                })
                .await
                .map_err(|error| error.to_string());
            ServerMsg::FileSearchResult { request, result }
        });
    }

    /// Searches GitHub pull requests and issues for a project's `#` mention
    /// drawer, placing pull requests ahead of issues.
    pub(super) fn search_github(&mut self, project: String, request: u64, query: String) {
        let client = self.client.clone();
        self.spawn_msg(async move {
            let pr_req = GitHubSearch {
                project: project.clone(),
                kind: GitHubItemKind::PullRequest,
                query: query.clone(),
                page: Some(1),
            };
            let issue_req = GitHubSearch {
                project,
                kind: GitHubItemKind::Issue,
                query,
                page: Some(1),
            };
            let (prs, issues) = tokio::join!(client.request(&pr_req), client.request(&issue_req));
            let result = match (prs, issues) {
                (Ok(mut pr_list), Ok(issue_list)) => {
                    pr_list.items.extend(issue_list.items);
                    Ok(pr_list.items)
                }
                (Ok(pr_list), Err(_)) => Ok(pr_list.items),
                (Err(_), Ok(issue_list)) => Ok(issue_list.items),
                (Err(error), Err(_)) => Err(error.to_string()),
            };
            ServerMsg::GitHubSearchResult { request, result }
        });
    }

    /// Promotes a queued input to steer immediately.
    pub(super) async fn steer_queued(&self, thread: ThreadId, run: RunId) -> Result<()> {
        self.client
            .request(&SteerQueuedInput { thread, run })
            .await
            .map(drop)
    }

    /// Updates a queued input's prompt text in place.
    pub(super) async fn update_queued(
        &self,
        thread: ThreadId,
        run: RunId,
        text: &str,
    ) -> Result<()> {
        self.client
            .request(&UpdateQueuedInput {
                thread,
                run,
                text: Some(text.to_owned()),
                before: Patch::Keep,
            })
            .await
            .map(drop)
    }

    /// Removes a queued input from the queue.
    pub(super) async fn remove_queued(&self, thread: ThreadId, run: RunId) -> Result<()> {
        self.client
            .remove_queued_input(&RemoveQueuedInput { thread, run })
            .await
    }
}
