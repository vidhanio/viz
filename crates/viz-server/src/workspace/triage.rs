//! Apply triage decisions, and let time pass for threads nobody touches.

use std::{sync::Arc, time::Duration};

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
    sql::{Connection, exec, row},
    store,
};
use crate::error::Error;

/// The longest wait between looks at the clock; a suspended machine may have
/// slept through a timer.
const LOOK: Duration = Duration::from_mins(1);

impl Workspace {
    pub(super) async fn triage(&self, request: TriageThread) -> Result<Triaged, Error> {
        let TriageThread { thread, action } = request;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let (prior, activity) = store::lifecycle(&mut tx, thread).await?;
        let decided = lifecycle::triage(&prior, action, activity.working, store::now())?;
        store::save_lifecycle(&mut tx, thread, &decided.lifecycle).await?;
        if let Some((list, position)) = decided.position {
            let (others, arranged) = others(&mut tx, thread, list).await?;
            let places = lifecycle::place(others, arranged, thread, position)?;
            let statement = match list {
                List::Pinned => "UPDATE threads SET pin = ?2 WHERE id = ?1",
                List::Active => "UPDATE threads SET sort_order = ?2 WHERE id = ?1",
            };
            for (thread, place) in places {
                exec(&mut tx, statement, [thread.into(), place.into()]).await?;
            }
        }
        let (lifecycle, _) = store::lifecycle(&mut tx, thread).await?;
        tx.commit().await?;
        drop(db);
        self.due.notify_one();
        self.relist(thread);
        Ok(Triaged { prior, lifecycle })
    }

    pub(super) async fn retitle(&self, request: RetitleThread) -> Result<ThreadUpdated, Error> {
        let RetitleThread { thread } = request;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        let (prior, _) = store::metadata(&mut tx, thread).await?;
        let mut sources = row(
            &mut tx,
            "SELECT json_extract(session, '$.title'), (SELECT text FROM runs
                WHERE thread = t.id AND kind = 'prompt' AND text <> '' ORDER BY id DESC LIMIT 1)
             FROM threads AS t WHERE id = ?1",
            [thread.into()],
        )
        .await?
        .ok_or(Error::NoThread(thread))?;
        let (harness, prompt) = (sources.opt_str(0)?, sources.opt_str(1)?);
        let next = threads::retitle(prior.clone(), harness.as_deref(), prompt.as_deref());
        store::save_metadata(&mut tx, thread, &next).await?;
        tx.commit().await?;
        drop(db);
        self.relist(thread);
        Ok(store::updated(&prior, &next))
    }

    /// Wake and settle threads as their time comes.
    pub(super) async fn keep_time(self: Arc<Self>) {
        loop {
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

    async fn pass_time(&self) -> Result<Duration, Error> {
        let now = store::now();
        let mut wait = LOOK;
        let mut changed = Vec::new();
        {
            let mut db = self.db.lock().await;
            let mut tx = db.transaction().await?;
            for mut thread in store::standings(&mut tx).await? {
                let policy = lifecycle::project_policy(thread.auto_settle);
                let current = &thread.lifecycle;
                thread.activity.pull_request = self.finished_pull(thread.thread, &thread.cwd).await;
                if let Some(next) = lifecycle::on_time(current, &thread.activity, policy, now) {
                    store::save_lifecycle(&mut tx, thread.thread, &next).await?;
                    changed.push(thread.thread);
                } else if let Some(until) = current.snoozed_until {
                    let left = u64::try_from(until - now).unwrap_or_default();
                    wait = wait.min(Duration::from_millis(left));
                }
            }
            tx.commit().await?;
            drop(db);
        }
        for thread in changed {
            self.relist(thread);
        }
        Ok(wait)
    }
}

/// The threads of a list from top to bottom, without `thread`, and how many of
/// them at the end have a position.
async fn others(
    db: &mut Connection,
    thread: ThreadId,
    list: List,
) -> Result<(Vec<ThreadId>, usize), Error> {
    let mut others = store::standings(db).await?;
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
