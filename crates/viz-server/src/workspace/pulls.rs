//! Batch GitHub branch lookups outside the workspace write lease.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use viz_git::{Head, RemoteRepository, Repository};
use viz_github::{PollState, PullRequest, poll_delay};
use viz_protocol::{GitHubPull, GitHubPullState, ThreadId};

use super::{Workspace, sql::rows};
use crate::error::Error;

#[derive(Default)]
pub(super) struct Pulls(Mutex<HashMap<ThreadId, Cached>>);

#[derive(Clone, PartialEq, Eq)]
struct Cached {
    cwd: String,
    pull: GitHubPull,
    finished_at: Option<i64>,
}

#[derive(Default)]
struct Backoff {
    failures: u32,
    retry: Option<Instant>,
}

impl Workspace {
    pub(super) fn pull_summary(&self, thread: ThreadId, cwd: &str) -> Option<GitHubPull> {
        self.pulls
            .0
            .lock()
            .expect("pull requests lock poisoned")
            .get(&thread)
            .filter(|cached| cached.cwd == cwd)
            .map(|cached| cached.pull.clone())
    }

    pub(super) async fn finished_pull(
        &self,
        thread: ThreadId,
        cwd: &str,
    ) -> Option<viz_core::lifecycle::PullRequest> {
        let cached = self
            .pulls
            .0
            .lock()
            .expect("pull requests lock poisoned")
            .get(&thread)
            .filter(|cached| cached.cwd == cwd && cached.finished_at.is_some())
            .cloned()?;
        let repo = Repository::discover(Path::new(cwd), &self.closing)
            .await
            .ok()?;
        let Head::Branch { name, .. } = repo.head(&self.closing).await.ok()? else {
            return None;
        };
        (name == cached.pull.head).then_some(viz_core::lifecycle::PullRequest {
            finished: true,
            changed_at: cached.finished_at?,
        })
    }

    pub(super) async fn poll_pulls(self: Arc<Self>) {
        let mut backoff = BTreeMap::new();
        loop {
            tokio::select! {
                () = self.closing.cancelled() => break,
                result = self.refresh_pulls(&mut backoff) => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "refreshing thread pull requests");
                    }
                }
            }
            tokio::select! {
                () = self.closing.cancelled() => break,
                () = self.pull_due.notified() => {},
                () = tokio::time::sleep(poll_delay(PollState::Active)) => {},
            }
        }
    }

    async fn refresh_pulls(
        &self,
        backoff: &mut BTreeMap<RemoteRepository, Backoff>,
    ) -> Result<(), Error> {
        let candidates = rows(&mut *self.db.lock().await,
            "SELECT id, cwd, archived, settled_at IS NOT NULL FROM threads WHERE project IS NOT NULL",
            []).await?;
        let mut known = HashMap::new();
        let mut checkouts: BTreeMap<String, Vec<ThreadId>> = BTreeMap::new();
        for mut row in candidates {
            let thread = row.id(0)?;
            let cwd = row.str(1)?;
            if viz_core::lifecycle::poll_pull_request(
                self.is_watched(thread),
                row.flag(2)?,
                row.flag(3)?,
            ) {
                checkouts.entry(cwd.clone()).or_default().push(thread);
            }
            known.insert(thread, cwd);
        }
        {
            let mut pulls = self.pulls.0.lock().expect("pull requests lock poisoned");
            pulls.retain(|thread, cached| known.get(thread) == Some(&cached.cwd));
        }
        let mut batches: BTreeMap<_, BTreeMap<String, Vec<ThreadId>>> = BTreeMap::new();
        for (cwd, threads) in checkouts {
            let resolved = async {
                let repo = Repository::discover(Path::new(&cwd), &self.closing)
                    .await
                    .ok()?;
                let Head::Branch { name, .. } = repo.head(&self.closing).await.ok()? else {
                    return None;
                };
                Some((self.github_remote(&repo).await.ok()?, name))
            }
            .await;
            let Some((remote, name)) = resolved else {
                for thread in threads {
                    self.update_pull(thread, &cwd, None);
                }
                continue;
            };
            for &thread in &threads {
                if self
                    .pull_summary(thread, &cwd)
                    .is_some_and(|pull| pull.head != name)
                {
                    self.update_pull(thread, &cwd, None);
                }
            }
            batches
                .entry(remote)
                .or_default()
                .entry(name)
                .or_default()
                .extend(threads);
        }
        backoff.retain(|remote, _| batches.contains_key(remote));
        for (remote, branches) in batches {
            let schedule = backoff.entry(remote.clone()).or_default();
            if schedule.retry.is_some_and(|retry| retry > Instant::now()) {
                continue;
            }
            let names: Vec<_> = branches.keys().map(String::as_str).collect();
            let result = async {
                let client = self.github(&remote.host).await?;
                for names in names.chunks(20) {
                    for (name, pull) in client.pulls_for_branches(&remote, names).await? {
                        for thread in &branches[&name] {
                            self.update_pull(*thread, &known[thread], pull.as_ref());
                        }
                    }
                }
                Ok::<(), Error>(())
            }
            .await;
            schedule.update(result);
        }
        Ok(())
    }

    fn update_pull(&self, thread: ThreadId, cwd: &str, remote: Option<&PullRequest>) {
        let new = remote.and_then(|pull| cached(pull, cwd));
        let changed = {
            let mut pulls = self.pulls.0.lock().expect("pull requests lock poisoned");
            if pulls.get(&thread) == new.as_ref() {
                false
            } else {
                if let Some(cached) = new {
                    pulls.insert(thread, cached);
                } else {
                    pulls.remove(&thread);
                }
                true
            }
        };
        if changed {
            self.relist(thread);
            self.due.notify_one();
        }
    }
}

impl Backoff {
    fn update(&mut self, result: Result<(), Error>) {
        let delay = match result {
            Ok(()) => {
                self.failures = 0;
                self.retry = None;
                return;
            }
            Err(Error::GitHub(viz_github::Error::RateLimited { retry_after })) => {
                poll_delay(PollState::RateLimited { retry_after })
            }
            Err(Error::GitHub(viz_github::Error::GhMissing | viz_github::Error::LoggedOut)) => {
                Duration::from_secs(120)
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                tracing::debug!(%error, "GitHub pull request polling will retry");
                poll_delay(PollState::Failed {
                    consecutive: self.failures,
                })
            }
        };
        self.retry = Instant::now().checked_add(delay);
    }
}

fn cached(pull: &PullRequest, cwd: &str) -> Option<Cached> {
    let (state, finished_at) = match pull.state.as_str() {
        "OPEN" => (GitHubPullState::Open, None),
        "MERGED" => (GitHubPullState::Merged, pull.merged_at.as_deref()),
        "CLOSED" => (GitHubPullState::Closed, pull.closed_at.as_deref()),
        _ => return None,
    };
    Some(Cached {
        cwd: cwd.to_owned(),
        finished_at: finished_at
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp_millis()),
        pull: GitHubPull {
            number: pull.number,
            title: pull.title.clone(),
            url: pull.url.clone(),
            state,
            draft: pull.draft,
            head: pull.head_ref_name.clone(),
            head_commit: pull.head_ref_oid.clone(),
            base: pull.base_ref_name.clone(),
            additions: pull.additions,
            deletions: pull.deletions,
            mergeable: pull.mergeable.clone(),
            checks: pull.checks_rollup.clone(),
            review: pull.review_decision.clone(),
        },
    })
}
