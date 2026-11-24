//! Apply triage decisions, and let time pass for threads nobody touches.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::time;
use viz_core::{
    lifecycle::{self, List},
    threads,
};
use viz_protocol::{
    Lifecycle, RetitleThread, Shelf, Standing, ThreadId, ThreadUpdated, TriageThread, Triaged,
};

use super::{
    Workspace,
    sql::{Conn, exec, row},
    store,
};
use crate::{acp::Housekeeping, error::Error};

/// The longest wait between looks at the clock; a suspended machine may have
/// slept through a timer.
const LOOK: Duration = Duration::from_mins(1);

impl Workspace {
    pub(super) async fn triage(&self, request: TriageThread) -> Result<Triaged, Error> {
        let TriageThread { thread, action } = request;
        let (prior, lifecycle, binding) = self
            .db
            .write_tx(move |tx| {
                let (prior, activity) = store::lifecycle(tx, thread)?;
                let decided = lifecycle::triage(&prior, action, activity.working, store::now())?;
                store::save_lifecycle(tx, thread, &decided.lifecycle)?;
                if let Some((list, position)) = decided.position {
                    let (others, arranged) = others(tx, thread, list)?;
                    let places = lifecycle::place(others, arranged, thread, position)?;
                    let statement = match list {
                        List::Pinned => "UPDATE threads SET pin = ?2 WHERE id = ?1",
                        List::Active => "UPDATE threads SET sort_order = ?2 WHERE id = ?1",
                    };
                    for (thread, place) in places {
                        exec(tx, statement, [thread.into(), place.into()])?;
                    }
                }
                let (lifecycle, _) = store::lifecycle(tx, thread)?;
                let binding = store::binding(tx, thread)?;
                Ok((prior, lifecycle, binding))
            })
            .await?;
        if lifecycle.archived != prior.archived {
            let command = if lifecycle.archived {
                Housekeeping::Archive
            } else {
                Housekeeping::Unarchive
            };
            self.housekeep(&binding, command);
        }
        self.due.notify_one();
        self.relist(thread);
        Ok(Triaged { prior, lifecycle })
    }

    pub(super) async fn retitle(&self, request: RetitleThread) -> Result<ThreadUpdated, Error> {
        let RetitleThread { thread } = request;
        let updated = self
            .db
            .write_tx(move |tx| {
                let (prior, _) = store::metadata(tx, thread)?;
                let mut sources = row(
                    tx,
                    "SELECT json_extract(session, '$.title'), (SELECT text FROM runs
                        WHERE thread = t.id AND kind = 'prompt' AND text <> '' ORDER BY id DESC LIMIT 1)
                     FROM threads AS t WHERE id = ?1",
                    [thread.into()],
                )?
                .ok_or(Error::NoThread(thread))?;
                let (harness, prompt) = (sources.opt_str(0)?, sources.opt_str(1)?);
                let next = threads::retitle(prior.clone(), harness.as_deref(), prompt.as_deref());
                store::save_metadata(tx, thread, &next)?;
                Ok(store::updated(&prior, &next))
            })
            .await?;
        self.relist(thread);
        Ok(updated)
    }

    /// Wake and settle threads as their time comes, running convergent
    /// reconciliation for checkpoints, blobs, and orphaned directories each
    /// cycle.
    pub(super) async fn keep_time(self: Arc<Self>) {
        loop {
            self.reconcile_periodic().await;
            let wait = match self.pass_time().await {
                Ok(wait) => wait,
                Err(error) => {
                    tracing::error!(%error, "settling and waking threads");
                    LOOK
                }
            };
            tokio::select! {
                () = self.closing.cancelled() => return,
                () = self.due.notified() => {},
                () = time::sleep(wait) => {},
            }
        }
    }

    /// Finished pull requests are checked against each checkout's `HEAD`
    /// before entering the SQLite writer; the decisions use the standings read
    /// inside the write transaction.
    async fn pass_time(&self) -> Result<Duration, Error> {
        let candidates = self.db.read(|conn| store::standings(conn)).await?;
        let mut pulls = HashMap::new();
        for thread in candidates {
            if let Some(pull) = self.finished_pull(thread.thread, &thread.cwd).await {
                pulls.insert(thread.thread, (thread.cwd, pull));
            }
        }
        let (wait, changed) = self
            .db
            .write_tx(move |tx| {
                let now = store::now();
                let mut wait = LOOK;
                let mut changed = Vec::new();
                for mut thread in store::standings(tx)? {
                    let policy = lifecycle::project_policy(thread.auto_settle);
                    let current = &thread.lifecycle;
                    thread.activity.pull_request = pulls
                        .remove(&thread.thread)
                        .filter(|(cwd, _)| *cwd == thread.cwd)
                        .map(|(_, pull)| pull);
                    if let Some(next) = lifecycle::on_time(current, &thread.activity, policy, now) {
                        store::save_lifecycle(tx, thread.thread, &next)?;
                        changed.push(thread.thread);
                    } else if let Some(until) = current.snoozed_until {
                        let left = u64::try_from(until - now).unwrap_or_default();
                        wait = wait.min(Duration::from_millis(left));
                    }
                }
                Ok((wait, changed))
            })
            .await?;
        for thread in changed {
            self.relist(thread);
        }
        Ok(wait)
    }
}

/// The threads of a list from top to bottom, without `thread`, and how many of
/// them at the end have a position.
fn others(db: &Conn, thread: ThreadId, list: List) -> Result<(Vec<ThreadId>, usize), Error> {
    let mut others = store::standings(db)?;
    let shelf = match list {
        List::Pinned => Shelf::Pinned,
        List::Active => Shelf::Active,
    };
    let position = |lifecycle: &Lifecycle| match list {
        List::Pinned => lifecycle.pin,
        List::Active => lifecycle.order,
    };
    others.retain(|other| {
        let open = !matches!(other.lifecycle.standing, Standing::Settled { .. });
        other.thread != thread
            && open
            && other.lifecycle.pin.is_some() == matches!(list, List::Pinned)
    });
    others.sort_by_cached_key(|other| {
        lifecycle::sort_key(shelf, &other.lifecycle, &other.activity, other.thread)
    });
    let arranged = others
        .iter()
        .filter(|other| position(&other.lifecycle).is_some())
        .count();
    Ok((
        others.into_iter().map(|other| other.thread).collect(),
        arranged,
    ))
}
