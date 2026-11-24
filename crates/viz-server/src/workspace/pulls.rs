//! Batch GitHub branch lookups persisted in SQLite `thread_pulls` outside any
//! database write lock.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use viz_git::{Head, RemoteRepository, Repository};
use viz_github::{PollState, PullRequest, poll_delay};
use viz_protocol::{GitHubPull, GitHubPullState, ThreadId};

use super::{
    Workspace,
    sql::{exec, row, rows},
    store,
};
use crate::error::Error;

type PullCandidate = (ThreadId, String, bool, bool);

#[derive(Clone, PartialEq, Eq)]
struct StoredPull {
    cwd: String,
    remote: RemoteRepository,
    pull: GitHubPull,
    finished_at: Option<i64>,
}

#[derive(Default)]
struct Backoff {
    failures: u32,
    retry: Option<Instant>,
}

impl Workspace {
    pub(super) async fn invalidate_checkout_pulls(&self, checkout: &Path) {
        let checkout = checkout.to_owned();
        let affected = self
            .db
            .write_tx(move |tx| {
                let existing: Vec<(ThreadId, String)> =
                    rows(tx, "SELECT thread, cwd FROM thread_pulls", [])?
                        .map(|mut row| Ok((row.id(0)?, row.str(1)?)))
                        .collect::<Result<_, Error>>()?;
                let mut removed = Vec::new();
                for (thread, cwd) in existing {
                    if Path::new(&cwd).starts_with(&checkout) {
                        exec(
                            tx,
                            "DELETE FROM thread_pulls WHERE thread = ?1",
                            [thread.into()],
                        )?;
                        removed.push(thread);
                    }
                }
                Ok(removed)
            })
            .await
            .unwrap_or_default();
        for thread in affected {
            self.relist(thread);
        }
        self.pull_due.notify_one();
    }

    pub(super) async fn finished_pull(
        &self,
        thread: ThreadId,
        cwd: &str,
    ) -> Option<viz_core::lifecycle::PullRequest> {
        let cwd_key = cwd.to_owned();
        let stored = self
            .db
            .read(move |conn| {
                let Some(mut record) = row(
                    conn,
                    "SELECT remote_host, remote_owner, remote_repo, pull_json, finished_at
                     FROM thread_pulls
                     WHERE thread = ?1 AND cwd = ?2 AND finished_at IS NOT NULL",
                    [thread.into(), cwd_key.as_str().into()],
                )?
                else {
                    return Ok(None);
                };
                let remote = RemoteRepository {
                    host: record.str(0)?,
                    owner: record.str(1)?,
                    repo: record.str(2)?,
                };
                let pull: GitHubPull = store::from_json(&record.str(3)?)?;
                let finished_at = record.opt_i64(4)?;
                Ok(Some((remote, pull, finished_at)))
            })
            .await
            .ok()??;
        let (remote, pull, finished_at) = stored;
        let repo = Repository::discover(Path::new(cwd), &self.closing)
            .await
            .ok()?;
        let Head::Branch { name, commit } = repo.head(&self.closing).await.ok()? else {
            return None;
        };
        (name == pull.head
            && commit == pull.head_commit
            && self.github_remote(&repo).await.ok()? == remote)
            .then_some(viz_core::lifecycle::PullRequest {
                finished: true,
                changed_at: finished_at?,
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

    async fn load_pull_snapshot(
        &self,
    ) -> Result<(Vec<PullCandidate>, HashMap<ThreadId, StoredPull>), Error> {
        self.db
            .read_snapshot(|tx| {
                let candidates: Vec<PullCandidate> = rows(
                    tx,
                    "SELECT id, cwd, archived, settled_at IS NOT NULL FROM threads WHERE project IS NOT NULL",
                    [],
                )?
                .map(|mut row| Ok((row.id(0)?, row.str(1)?, row.flag(2)?, row.flag(3)?)))
                .collect::<Result<_, Error>>()?;
                let mut existing = HashMap::new();
                for mut record in rows(
                    tx,
                    "SELECT thread, cwd, remote_host, remote_owner, remote_repo, pull_json, finished_at
                     FROM thread_pulls",
                    [],
                )? {
                    let thread: ThreadId = record.id(0)?;
                    let cwd = record.str(1)?;
                    let remote = RemoteRepository {
                        host: record.str(2)?,
                        owner: record.str(3)?,
                        repo: record.str(4)?,
                    };
                    if let Ok(pull) = store::from_json(&record.str(5)?) {
                        existing.insert(
                            thread,
                            StoredPull {
                                cwd,
                                remote,
                                pull,
                                finished_at: record.opt_i64(6)?,
                            },
                        );
                    }
                }
                Ok((candidates, existing))
            })
            .await
    }

    async fn refresh_pulls(
        &self,
        backoff: &mut BTreeMap<RemoteRepository, Backoff>,
    ) -> Result<(), Error> {
        let (candidates, mut existing) = self.load_pull_snapshot().await?;

        let mut known = HashMap::new();
        let mut checkouts: BTreeMap<String, Vec<ThreadId>> = BTreeMap::new();
        for (thread, cwd, archived, settled) in candidates {
            if viz_core::lifecycle::poll_pull_request(self.is_watched(thread), archived, settled) {
                checkouts.entry(cwd.clone()).or_default().push(thread);
            }
            known.insert(thread, cwd);
        }

        let stale_threads: Vec<ThreadId> = existing
            .iter()
            .filter(|(thread, stored)| known.get(*thread) != Some(&stored.cwd))
            .map(|(thread, _)| *thread)
            .collect();
        for thread in stale_threads {
            existing.remove(&thread);
            self.write_pull(thread, None).await?;
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
                    if existing.remove(&thread).is_some() {
                        self.write_pull(thread, None).await?;
                    }
                }
                continue;
            };
            for &thread in &threads {
                let stale = existing
                    .get(&thread)
                    .is_some_and(|stored| stored.pull.head != name || stored.remote != remote);
                if stale {
                    existing.remove(&thread);
                    self.write_pull(thread, None).await?;
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
                            let next = pull
                                .as_ref()
                                .and_then(|pull| stored_pull(pull, &known[thread], &remote));
                            if existing.get(thread) != next.as_ref() {
                                if let Some(ref item) = next {
                                    existing.insert(*thread, item.clone());
                                } else {
                                    existing.remove(thread);
                                }
                                self.write_pull(*thread, next).await?;
                            }
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

    async fn write_pull(&self, thread: ThreadId, next: Option<StoredPull>) -> Result<(), Error> {
        let changed = self
            .db
            .write(move |conn| {
                if let Some(stored) = next {
                    let pull_json = store::json(&stored.pull)?;
                    let count = exec(
                        conn,
                        "INSERT INTO thread_pulls (thread, cwd, remote_host, remote_owner, remote_repo, pull_json, finished_at)
                         SELECT id, ?2, ?3, ?4, ?5, ?6, ?7 FROM threads WHERE id = ?1
                         ON CONFLICT(thread) DO UPDATE SET
                             cwd = excluded.cwd,
                             remote_host = excluded.remote_host,
                             remote_owner = excluded.remote_owner,
                             remote_repo = excluded.remote_repo,
                             pull_json = excluded.pull_json,
                             finished_at = excluded.finished_at",
                        [
                            thread.into(),
                            stored.cwd.into(),
                            stored.remote.host.into(),
                            stored.remote.owner.into(),
                            stored.remote.repo.into(),
                            pull_json.into(),
                            stored.finished_at.into(),
                        ],
                    )?;
                    Ok(count > 0)
                } else {
                    let count = exec(
                        conn,
                        "DELETE FROM thread_pulls WHERE thread = ?1",
                        [thread.into()],
                    )?;
                    Ok(count > 0)
                }
            })
            .await?;
        if changed {
            self.relist(thread);
            self.due.notify_one();
        }
        Ok(())
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

fn stored_pull(pull: &PullRequest, cwd: &str, remote: &RemoteRepository) -> Option<StoredPull> {
    let (state, finished_at) = match pull.state.as_str() {
        "OPEN" => (GitHubPullState::Open, None),
        "MERGED" => (GitHubPullState::Merged, pull.merged_at.as_deref()),
        "CLOSED" => (GitHubPullState::Closed, pull.closed_at.as_deref()),
        _ => return None,
    };
    Some(StoredPull {
        cwd: cwd.to_owned(),
        remote: remote.clone(),
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
