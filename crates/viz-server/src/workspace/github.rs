//! One authenticated client per GitHub host that the workspace uses.

use viz_protocol::{GitHubRepositories, GitHubRepository, GitHubRepositoryList};

use super::Workspace;
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
