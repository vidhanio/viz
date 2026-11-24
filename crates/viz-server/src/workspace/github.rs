//! One authenticated client per GitHub host that the workspace uses.

use std::{path::Path, sync::Arc};

use viz_git::{Head, Repository};
use viz_github::{IssueKind, RemoteRepository};
use viz_protocol::{
    GitHubAuth, GitHubAuthState, GitHubCheck, GitHubChecks, GitHubCreatePull, GitHubItem,
    GitHubItemKind, GitHubLaunch, GitHubPullPage, GitHubRepositories, GitHubRepository,
    GitHubRepositoryList, GitHubReview, GitHubSearch, GitHubSearchResult,
};

use super::{Workspace, store};
use crate::error::Error;

impl Workspace {
    pub(super) async fn github(&self, host: &str) -> Result<viz_github::GitHub, Error> {
        let mut clients = self.github.lock().await;
        if let Some(client) = clients.get(host) {
            return Ok(client.clone());
        }
        let client = viz_github::GitHub::new(host)?;
        clients.insert(client.host().to_owned(), client.clone());
        drop(clients);
        Ok(client)
    }

    pub(super) async fn github_repositories(
        &self,
        args: GitHubRepositories,
    ) -> Result<GitHubRepositoryList, Error> {
        let client = self
            .github(args.host.as_deref().unwrap_or("github.com"))
            .await?;
        let repositories = client
            .repositories(args.query.as_deref(), args.page.unwrap_or(1))
            .await?
            .into_iter()
            .map(|repo| GitHubRepository {
                name: repo.name,
                full_name: repo.full_name,
                default_branch: repo.default_branch,
                clone_url: repo.clone_url,
                ssh_url: repo.ssh_url,
                avatar_url: repo.owner.avatar_url,
                private: repo.private,
            })
            .collect();
        Ok(GitHubRepositoryList { repositories })
    }
}

impl Workspace {
    pub(super) async fn github_auth(&self, args: GitHubAuth) -> Result<GitHubAuthState, Error> {
        let client = self
            .github(args.host.as_deref().unwrap_or("github.com"))
            .await?;
        Ok(match client.auth_state().await? {
            viz_github::AuthState::Ready => GitHubAuthState::Ready,
            viz_github::AuthState::GhMissing => GitHubAuthState::GhMissing,
            viz_github::AuthState::LoggedOut => GitHubAuthState::LoggedOut,
        })
    }

    async fn github_project(&self, project: &str) -> Result<RemoteRepository, Error> {
        let project = project.to_owned();
        let project = self
            .db
            .read(move |conn| store::require_project(conn, &project))
            .await?;
        let repo = Repository::discover(Path::new(&project), &self.closing).await?;
        self.github_remote(&repo).await
    }

    pub(super) async fn github_remote(&self, repo: &Repository) -> Result<RemoteRepository, Error> {
        repo.remotes(&self.closing)
            .await?
            .into_iter()
            .filter_map(|remote| {
                remote
                    .repository
                    .map(|repository| (remote.name, repository))
            })
            .min_by(|(a, _), (b, _)| (a != "origin", a).cmp(&(b != "origin", b)))
            .map(|(_, repository)| repository)
            .ok_or_else(|| Error::not_found("checkout has no GitHub repository remote"))
    }

    pub(super) async fn github_search(
        &self,
        args: GitHubSearch,
    ) -> Result<GitHubSearchResult, Error> {
        let remote = self.github_project(&args.project).await?;
        let client = self.github(&remote.host).await?;
        let query = args.query.trim().trim_start_matches('#');
        let kind = match args.kind {
            GitHubItemKind::Issue => IssueKind::Issue,
            GitHubItemKind::PullRequest => IssueKind::PullRequest,
        };
        if let Ok(number) = query.parse::<u64>() {
            let items = match client.issue(&remote, number).await {
                Ok(detail) if detail.is_pull() == (kind == IssueKind::PullRequest) => {
                    vec![item(detail.item, args.kind)]
                }
                Ok(_) => Vec::new(),
                Err(viz_github::Error::Http(status)) if status.as_u16() == 404 => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            return Ok(GitHubSearchResult { items });
        }
        let items = client
            .mentions(&remote, kind, query, args.page.unwrap_or(1))
            .await?
            .into_iter()
            .map(|found| item(found, args.kind))
            .collect();
        Ok(GitHubSearchResult { items })
    }

    pub(super) async fn github_create_pull(
        &self,
        args: GitHubCreatePull,
    ) -> Result<GitHubItem, Error> {
        let repo = self.git_repository(args.thread).await?;
        let Head::Branch { name, .. } = repo.head(&self.closing).await? else {
            return Err(viz_git::Error::Invalid("attached branch with commits").into());
        };
        let remote = self.github_remote(&repo).await?;
        let client = self.github(&remote.host).await?;
        let input = viz_github::CreatePullRequest {
            head: &name,
            base: args.base.as_deref(),
            title: &args.title,
            body: &args.body,
            mode: if args.draft {
                viz_github::PullRequestMode::Draft
            } else {
                viz_github::PullRequestMode::Ready
            },
        };
        let issue = client.create_pull(&remote, input).await?;
        self.pull_due.notify_one();
        Ok(item(issue, GitHubItemKind::PullRequest))
    }

    pub(super) async fn github_checks(
        &self,
        args: GitHubChecks,
    ) -> Result<Vec<GitHubCheck>, Error> {
        let remote = self
            .github_remote(&self.git_repository(args.thread).await?)
            .await?;
        let client = self.github(&remote.host).await?;
        Ok(client
            .check_runs(&remote, &args.commit, args.page.unwrap_or(1))
            .await?
            .into_iter()
            .map(|check| GitHubCheck {
                id: check.id,
                name: check.name,
                status: check.status,
                conclusion: check.conclusion,
                url: check.details_url,
            })
            .collect())
    }

    pub(super) async fn github_reviews(
        &self,
        args: GitHubPullPage,
    ) -> Result<Vec<GitHubReview>, Error> {
        let remote = self
            .github_remote(&self.git_repository(args.thread).await?)
            .await?;
        let client = self.github(&remote.host).await?;
        Ok(client
            .reviews(&remote, args.number, args.page.unwrap_or(1))
            .await?
            .into_iter()
            .map(|review| GitHubReview {
                id: review.id,
                author: review.user.map(|user| user.login),
                state: review.state,
                body: review.body,
                url: review.html_url,
                submitted_at: review.submitted_at,
            })
            .collect())
    }

    pub(super) async fn github_launch(
        self: &Arc<Self>,
        principal_key: String,
        args: GitHubLaunch,
    ) -> Result<viz_protocol::Launched, Error> {
        let mut launch = args.launch;
        let project = launch
            .project
            .as_deref()
            .ok_or_else(|| Error::invalid("GitHub thread requires a project"))?;
        let remote = self.github_project(project).await?;
        let client = self.github(&remote.host).await?;
        let detail = client.issue(&remote, args.number).await?;
        let kind = if detail.is_pull() {
            "pull request"
        } else {
            "issue"
        };
        let issue = detail.item;
        let body = detail.body.as_deref().unwrap_or_default();
        let body: String = body.chars().take(32_000).collect();
        let mut prompt = launch
            .prompt
            .take()
            .unwrap_or_else(|| viz_protocol::Prompt::new(format!("Review this GitHub {kind}.")));
        let label = format!("GitHub {kind} #{} ({})", issue.number, issue.title);
        let kind = if kind == "pull request" {
            GitHubItemKind::PullRequest
        } else {
            GitHubItemKind::Issue
        };
        prompt.append_reference(
            &label,
            viz_protocol::PromptRef::GitHub {
                item: item(issue, kind),
            },
        );
        prompt.text.push_str("\n\n");
        prompt.text.push_str(&body);
        launch.prompt = Some(prompt);
        self.launch(principal_key, launch).await
    }
}

fn item(issue: viz_github::Issue, kind: GitHubItemKind) -> GitHubItem {
    GitHubItem {
        kind,
        number: issue.number,
        title: issue.title,
        url: issue.html_url,
        state: issue.state,
    }
}
