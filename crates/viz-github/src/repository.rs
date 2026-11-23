//! Repository, pull request, issue and account operations.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use url::form_urlencoded;

use crate::{
    Account, CheckRun, CreatePullRequest, Error, GitHub, Issue, IssueDetail, IssueKind,
    PullComment, PullRequest, PullRequestMode, PullReview, RemoteRepository, Repository, Template,
    client::missing_graphql,
};

const PR_FIELDS: &str = "
    number title url state isDraft headRefName headRefOid baseRefName baseRefOid mergeable reviewDecision
    additions deletions updatedAt closedAt mergedAt author { login avatarUrl }
    statusCheckRollup { state }
    headRepository { nameWithOwner }
    repository { nameWithOwner }
";

impl GitHub {
    /// Resolve a repository from an already parsed Git remote on this host.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn repository(&self, repository: &RemoteRepository) -> Result<Repository, Error> {
        self.get(&self.repo_route(repository)?).await
    }

    /// Return the authenticated viewer, including their avatar URL.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn viewer(&self) -> Result<Account, Error> {
        self.get("/user").await
    }

    /// Look up a project's owner avatar URL.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn owner_image(&self, repository: &RemoteRepository) -> Result<String, Error> {
        Ok(self.repository(repository).await?.owner.avatar_url)
    }

    /// Resolve HTTPS and SSH clone URLs, the default branch, and other
    /// repository metadata.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn clone_urls(&self, repository: &RemoteRepository) -> Result<Repository, Error> {
        self.repository(repository).await
    }

    /// List a page of repositories accessible to the viewer, or search
    /// repositories they own.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn repositories(
        &self,
        search: Option<&str>,
        page: u32,
    ) -> Result<Vec<Repository>, Error> {
        match search {
            Some(search) if !search.trim().is_empty() => {
                let viewer = self.viewer().await?;
                let route = query(
                    "/search/repositories",
                    page,
                    &[("q", &format!("user:{} {}", viewer.login, search.trim()))],
                )?;
                let result: Search<Repository> = self.get(&route).await?;
                Ok(result.items)
            }
            _ => {
                self.get(&query(
                    "/user/repos",
                    page,
                    &[("sort", "updated"), ("affiliation", "owner")],
                )?)
                .await
            }
        }
    }

    /// Find the current PR for a same-repository branch, preferring open over
    /// historical PRs.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn pull_for_branch(
        &self,
        repository: &RemoteRepository,
        branch: &str,
    ) -> Result<Option<PullRequest>, Error> {
        self.repo_route(repository)?;
        let query = format!("query($owner:String!, $repo:String!, $head:String!, $cursor:String) {{
            repository(owner:$owner, name:$repo) {{
                pullRequests(first:30, after:$cursor, headRefName:$head, orderBy:{{field:UPDATED_AT,direction:DESC}}) {{
                    pageInfo {{ endCursor hasNextPage }}
                    nodes {{ {PR_FIELDS} }}
                }}
            }}
        }}");
        let full_name = format!("{}/{}", repository.owner, repository.repo);
        let mut cursor = None;
        let mut historical = None;
        loop {
            let request = json!({"query":query, "variables":{
                "owner":repository.owner, "repo":repository.repo, "head":branch, "cursor":cursor
            }});
            let response: BranchRepository = self.graphql(&request).await?;
            let pulls = response
                .repository
                .ok_or_else(|| missing_graphql("repository unavailable"))?
                .pull_requests;
            if let Some(pull) = preferred_pull(pulls.nodes, &full_name) {
                if pull.state == "OPEN" {
                    return Ok(Some(pull));
                }
                if historical.is_none() {
                    historical = Some(pull);
                }
            }
            if !pulls.page_info.has_next_page {
                return Ok(historical);
            }
            cursor = Some(
                pulls
                    .page_info
                    .end_cursor
                    .ok_or_else(|| missing_graphql("missing pull request page cursor"))?,
            );
        }
    }

    /// Look up up to 100 distinct branches in one GraphQL request.
    ///
    /// Each requested branch has an entry, with open PRs preferred over
    /// historical PRs, including PRs whose remote branch was deleted.
    ///
    /// # Errors
    /// Returns an error if the batch exceeds 100 branches or GitHub rejects the
    /// request.
    pub async fn pulls_for_branches(
        &self,
        repository: &RemoteRepository,
        branches: &[&str],
    ) -> Result<BTreeMap<String, Option<PullRequest>>, Error> {
        self.repo_route(repository)?;
        let branches: BTreeSet<_> = branches.iter().copied().collect();
        if branches.len() > 100 {
            return Err(Error::Invalid(
                "pull request batch larger than 100 branches",
            ));
        }
        if branches.is_empty() {
            return Ok(BTreeMap::new());
        }
        let request = batch_query(repository, &branches)?;
        let response: BatchRepository = self.graphql(&request).await?;
        let mut results = response
            .repository
            .ok_or_else(|| missing_graphql("repository unavailable"))?;
        let full_name = format!("{}/{}", repository.owner, repository.repo);
        branches
            .into_iter()
            .enumerate()
            .map(|(index, branch)| {
                let open = results
                    .remove(&format!("open{index}"))
                    .ok_or_else(|| missing_graphql("missing open pull requests"))?;
                let recent = results
                    .remove(&format!("recent{index}"))
                    .ok_or_else(|| missing_graphql("missing historical pull requests"))?;
                let pull = preferred_pull(open.nodes, &full_name)
                    .or_else(|| preferred_pull(recent.nodes, &full_name));
                Ok((branch.to_owned(), pull))
            })
            .collect()
    }

    /// List or search a repository's issues or PRs for mention completion.
    /// Pages are one-based. An empty query includes open and closed items.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn mentions(
        &self,
        repository: &RemoteRepository,
        kind: IssueKind,
        search: &str,
        page: u32,
    ) -> Result<Vec<Issue>, Error> {
        self.repo_route(repository)?;
        let kind = match kind {
            IssueKind::Issue => "issue",
            IssueKind::PullRequest => "pr",
        };
        let term = format!(
            "repo:{}/{} is:{kind} {}",
            repository.owner,
            repository.repo,
            search.trim()
        );
        let path = query("/search/issues", page, &[("q", &term), ("sort", "updated")])?;
        let result: Search<Issue> = self.get(&path).await?;
        Ok(result.items)
    }

    /// Read the body and metadata of an issue or pull request.
    ///
    /// # Errors
    /// Returns an error if GitHub cannot resolve the item.
    pub async fn issue(
        &self,
        repository: &RemoteRepository,
        number: u64,
    ) -> Result<IssueDetail, Error> {
        let route = self.repo_route(repository)?;
        self.get(&format!("{route}/issues/{number}")).await
    }

    /// Create a PR. If base is omitted GitHub's configured default branch is
    /// used. The body is supplied by the caller, which can offer a discovered
    /// template when editing.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn create_pull(
        &self,
        repository: &RemoteRepository,
        input: CreatePullRequest<'_>,
    ) -> Result<Issue, Error> {
        let route = format!("{}/pulls", self.repo_route(repository)?);
        let default_branch;
        let base = if let Some(base) = input.base {
            base
        } else {
            default_branch = self.repository(repository).await?.default_branch;
            &default_branch
        };
        let body = json!({
            "head":input.head, "base":base, "title":input.title, "body":input.body,
            "draft":input.mode == PullRequestMode::Draft,
        });
        let pull = self.post(&self.rest(&route), &body).await?;
        self.invalidate_pull_search().await;
        Ok(pull)
    }

    /// Discover PR templates in the root, docs, and .github on the default
    /// branch.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn templates(&self, repository: &RemoteRepository) -> Result<Vec<Template>, Error> {
        let route = format!("{}/contents", self.repo_route(repository)?);
        let mut templates = Vec::new();
        for parent in ["/", "/docs", "/.github"] {
            for entry in self.contents(&format!("{route}{parent}")).await? {
                if entry.kind == "file"
                    && entry.name.eq_ignore_ascii_case("pull_request_template.md")
                {
                    templates.push(Template { path: entry.path });
                } else if entry.kind == "dir"
                    && entry.name.eq_ignore_ascii_case("PULL_REQUEST_TEMPLATE")
                {
                    let directory = path_suffix(&entry.path)?;
                    for template in self.contents(&format!("{route}{directory}")).await? {
                        if template.kind == "file"
                            && template.name.to_ascii_lowercase().ends_with(".md")
                        {
                            templates.push(Template {
                                path: template.path,
                            });
                        }
                    }
                }
            }
        }
        Ok(templates)
    }

    /// Read the Markdown of a template on the default branch.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn template_body(
        &self,
        repository: &RemoteRepository,
        template: &Template,
    ) -> Result<String, Error> {
        let route = format!(
            "{}/contents{}",
            self.repo_route(repository)?,
            path_suffix(&template.path)?
        );
        let raw = self
            .get_body(&route, Some("application/vnd.github.raw+json"))
            .await?;
        Ok(raw.to_string())
    }

    /// Read a page of conversation comments.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn pull_comments(
        &self,
        repository: &RemoteRepository,
        number: u64,
        page: u32,
    ) -> Result<Vec<PullComment>, Error> {
        let route = self.repo_route(repository)?;
        self.get(&query(
            &format!("{route}/issues/{number}/comments"),
            page,
            &[],
        )?)
        .await
    }

    /// Read a page of inline review comments.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn review_comments(
        &self,
        repository: &RemoteRepository,
        number: u64,
        page: u32,
    ) -> Result<Vec<PullComment>, Error> {
        let route = self.repo_route(repository)?;
        self.get(&query(
            &format!("{route}/pulls/{number}/comments"),
            page,
            &[],
        )?)
        .await
    }

    /// Read a page of reviews with their decisions.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn reviews(
        &self,
        repository: &RemoteRepository,
        number: u64,
        page: u32,
    ) -> Result<Vec<PullReview>, Error> {
        let route = self.repo_route(repository)?;
        self.get(&query(
            &format!("{route}/pulls/{number}/reviews"),
            page,
            &[],
        )?)
        .await
    }

    /// Read a page of check runs for a commit SHA.
    ///
    /// # Errors
    /// Returns an error if authentication fails or GitHub rejects the request.
    pub async fn check_runs(
        &self,
        repository: &RemoteRepository,
        commit: &str,
        page: u32,
    ) -> Result<Vec<CheckRun>, Error> {
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Invalid("commit SHA"));
        }
        let route = self.repo_route(repository)?;
        let result: CheckRuns = self
            .get(&query(
                &format!("{route}/commits/{commit}/check-runs"),
                page,
                &[],
            )?)
            .await?;
        Ok(result.check_runs)
    }

    fn repo_route(&self, repository: &RemoteRepository) -> Result<String, Error> {
        if !self.host().eq_ignore_ascii_case(&repository.host) {
            return Err(Error::Invalid("repository host"));
        }
        if ![&repository.owner, &repository.repo]
            .into_iter()
            .all(|part| {
                !matches!(part.as_str(), "" | "." | "..")
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            })
        {
            return Err(Error::Invalid("repository name"));
        }
        Ok(format!("/repos/{}/{}", repository.owner, repository.repo))
    }

    async fn contents(&self, route: &str) -> Result<Vec<Content>, Error> {
        match self.get(route).await {
            Err(Error::Http(StatusCode::NOT_FOUND)) => Ok(Vec::new()),
            result => result,
        }
    }
}

fn path_suffix(path: &str) -> Result<String, Error> {
    let mut suffix = String::new();
    for part in path.split('/') {
        if matches!(part, "" | "." | "..") {
            return Err(Error::Invalid("template path"));
        }
        suffix.push('/');
        suffix.extend(
            url::form_urlencoded::byte_serialize(part.as_bytes()).flat_map(|encoded| {
                if encoded == "+" {
                    "%20".chars()
                } else {
                    encoded.chars()
                }
            }),
        );
    }
    Ok(suffix)
}

fn query(path: &str, page: u32, params: &[(&str, &str)]) -> Result<String, Error> {
    if page == 0 {
        return Err(Error::Invalid("page number"));
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("page", &page.to_string())
        .append_pair("per_page", "30")
        .extend_pairs(params.iter().copied())
        .finish();
    Ok(format!("{path}?{query}"))
}

#[derive(Deserialize)]
struct Search<T> {
    items: Vec<T>,
}

#[derive(Deserialize)]
struct CheckRuns {
    check_runs: Vec<CheckRun>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    name: String,
}

fn batch_query(
    repository: &RemoteRepository,
    branches: &BTreeSet<&str>,
) -> Result<serde_json::Value, Error> {
    let mut query = "query($owner:String!, $repo:String!".to_owned();
    let mut selections = String::new();
    let mut variables = serde_json::Map::new();
    variables.insert("owner".to_owned(), json!(repository.owner));
    variables.insert("repo".to_owned(), json!(repository.repo));
    for (index, branch) in branches.iter().enumerate() {
        write!(query, ",$head{index}:String!").map_err(|_| Error::Invalid("batch query"))?;
        write!(
            selections,
            "open{index}:pullRequests(headRefName:$head{index},first:15,states:[OPEN],orderBy:{{field:UPDATED_AT,direction:DESC}}) {{
                nodes {{ {PR_FIELDS} }}
            }}
            recent{index}:pullRequests(headRefName:$head{index},first:15,orderBy:{{field:UPDATED_AT,direction:DESC}}) {{
                nodes {{ {PR_FIELDS} }}
            }} "
        )
        .map_err(|_| Error::Invalid("batch query"))?;
        variables.insert(format!("head{index}"), json!(branch));
    }
    write!(
        query,
        ") {{ repository(owner:$owner,name:$repo) {{ {selections} }} }}"
    )
    .map_err(|_| Error::Invalid("batch query"))?;
    Ok(json!({"query":query,"variables":variables}))
}

fn preferred_pull(nodes: Vec<Option<PullNode>>, full_name: &str) -> Option<PullRequest> {
    let mut historical = None;
    for node in nodes.into_iter().flatten().filter(|node| {
        node.head_repository
            .as_ref()
            .is_some_and(|head| head.name_with_owner.eq_ignore_ascii_case(full_name))
            && node
                .repository
                .name_with_owner
                .eq_ignore_ascii_case(full_name)
    }) {
        if node.pull.state == "OPEN" {
            return Some(node.finish());
        }
        if historical.is_none() {
            historical = Some(node.finish());
        }
    }
    historical
}

#[derive(Deserialize)]
struct BatchRepository {
    repository: Option<BTreeMap<String, PullCandidates>>,
}

#[derive(Deserialize)]
struct PullCandidates {
    nodes: Vec<Option<PullNode>>,
}

#[derive(Deserialize)]
struct BranchRepository {
    repository: Option<PullRequests>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequests {
    pull_requests: PullNodes,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullNodes {
    nodes: Vec<Option<PullNode>>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    end_cursor: Option<String>,
    has_next_page: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullNode {
    #[serde(flatten)]
    pull: PullRequest,
    status_check_rollup: Option<Rollup>,
    head_repository: Option<NamedRepository>,
    repository: NamedRepository,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamedRepository {
    name_with_owner: String,
}

impl PullNode {
    fn finish(self) -> PullRequest {
        PullRequest {
            checks_rollup: self.status_check_rollup.map(|rollup| rollup.state),
            ..self.pull
        }
    }
}
