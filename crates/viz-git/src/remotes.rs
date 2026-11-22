//! Remote identities and tracking graph.

use git2::BranchType;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{Error, Repository, branches::upstream, repository::head_unborn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepository {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// Parsed identity only. Remote URLs can contain credentials.
    pub repository: Option<RemoteRepository>,
}

/// Parse an HTTPS, SSH, git, or SCP-style remote without retaining credentials.
#[must_use]
pub fn parse_remote(remote: &str) -> Option<RemoteRepository> {
    let url = if remote.contains("://") {
        Url::parse(remote).ok()?
    } else {
        let (host, path) = remote.split_once(':')?;
        if host.contains('/') || host.contains('\\') || path.starts_with('/') {
            return None;
        }
        Url::parse(&format!("ssh://{host}/{path}")).ok()?
    };
    if !matches!(url.scheme(), "https" | "http" | "ssh" | "git") {
        return None;
    }
    let mut parts = url.path().trim_matches('/').split('/');
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(RemoteRepository {
        host: url.host_str()?.to_owned(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

impl Repository {
    /// Count commits unique to the current branch and its upstream.
    ///
    /// # Errors
    /// Returns an error if the upstream or commit graph cannot be read.
    pub async fn ahead_behind(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<AheadBehind>, Error> {
        self.read(cancel, |repo, _| {
            if head_unborn(repo)? || repo.head_detached()? {
                return Ok(None);
            }
            let head = repo.head()?;
            let local = repo.find_branch(head.shorthand()?, BranchType::Local)?;
            let Some(upstream) = upstream(&local)? else {
                return Ok(None);
            };
            let (ahead, behind) = repo.graph_ahead_behind(
                head.peel_to_commit()?.id(),
                upstream.get().peel_to_commit()?.id(),
            )?;
            Ok(Some(AheadBehind { ahead, behind }))
        })
        .await
    }

    /// List configured remote names and parsed repository identities.
    ///
    /// # Errors
    /// Returns an error if remote configuration cannot be read.
    pub async fn remotes(&self, cancel: &CancellationToken) -> Result<Vec<Remote>, Error> {
        self.read(cancel, |repo, _| {
            repo.remotes()?
                .iter()
                .filter_map(|name| name.ok().flatten())
                .map(|name| {
                    let remote = repo.find_remote(name)?;
                    Ok(Remote {
                        name: name.to_owned(),
                        repository: std::str::from_utf8(remote.url_bytes())
                            .ok()
                            .and_then(parse_remote),
                    })
                })
                .collect()
        })
        .await
    }
}
