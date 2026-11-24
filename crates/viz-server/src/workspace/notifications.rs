//! Thread watches, zero-SQLite live delta fan-out, and monotonic workspace
//! entity revision notifications.
//!
//! Durable changes are published after commit. Watch and live state updates
//! that must agree with persisted rows run on the SQLite writer thread
//! immediately after commit.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::broadcast;
use viz_protocol::{LiveDelta, TaskId};

use super::{Workspace, sql::exec, store};
use crate::{
    error::Error,
    wire::{LiveTurn, ThreadId},
};

/// One committed entity change advancing the workspace's monotonic revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EntityChange {
    ThreadUpsert(ThreadId),
    ThreadRemoved(ThreadId),
    ProjectUpsert(String),
    ProjectRemoved(String),
    TaskUpsert(TaskId),
    TaskRemoved(TaskId),
    Resync,
}

#[derive(Debug, Clone)]
pub(crate) enum Change {
    /// Live uncommitted ACP tail delta for a watched thread (zero SQLite or Git
    /// work required on delivery).
    Live {
        thread: ThreadId,
        delta: Arc<LiveDelta>,
    },
    /// Persisted entries or thread state changed for `thread`.
    Thread(ThreadId),
    /// A run settled on `thread` (wakes `wait_run` and refreshes watchers).
    Settled(ThreadId),
    /// Committed workspace directory mutation advancing `rev` from `base_rev`.
    Entity {
        base_rev: u64,
        rev: u64,
        change: EntityChange,
    },
    GitActivity,
    Harnesses,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum WatchChange {
    Open,
    Close,
}

#[derive(Default)]
struct Views {
    /// Trimmed live snapshot per thread (computed once at publish time instead
    /// of re-trimming per watcher).
    preview_live: HashMap<ThreadId, LiveTurn>,
    /// Unmodified raw live snapshot per thread for `thread.entry_chunk`.
    raw_live: HashMap<ThreadId, LiveTurn>,
    watches: HashMap<ThreadId, usize>,
}

pub(super) struct Updates {
    changes: broadcast::Sender<Change>,
    views: Mutex<Views>,
    rev: AtomicU64,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            changes: broadcast::channel(1024).0,
            views: Mutex::default(),
            rev: AtomicU64::new(1),
        }
    }
}

impl Updates {
    /// Updates the in-memory live snapshot for `thread` and computes the
    /// trimmed [`LiveDelta`] once for all watchers.
    pub(super) fn live(&self, thread: ThreadId, live: Option<LiveTurn>) -> Option<Arc<LiveDelta>> {
        let preview = live.clone().map(store::live_preview);
        let mut views = self.views.lock().expect("thread views poisoned");
        let prior = views.preview_live.get(&thread);
        let changed = prior != preview.as_ref();
        let delta = changed.then(|| Arc::new(LiveDelta::between(prior, preview.as_ref())));
        if let (Some(raw), Some(preview)) = (live, preview) {
            views.raw_live.insert(thread, raw);
            views.preview_live.insert(thread, preview);
        } else {
            views.raw_live.remove(&thread);
            views.preview_live.remove(&thread);
        }
        drop(views);
        delta
    }

    pub(super) fn latest(&self, thread: ThreadId) -> Option<LiveTurn> {
        self.views
            .lock()
            .expect("thread views poisoned")
            .preview_live
            .get(&thread)
            .cloned()
    }

    pub(super) fn raw_live(&self, thread: ThreadId) -> Option<LiveTurn> {
        self.views
            .lock()
            .expect("thread views poisoned")
            .raw_live
            .get(&thread)
            .cloned()
    }

    pub(super) fn remove(&self, thread: ThreadId) {
        let mut views = self.views.lock().expect("thread views poisoned");
        views.preview_live.remove(&thread);
        views.raw_live.remove(&thread);
        views.watches.remove(&thread);
    }

    pub(super) fn open_watch(&self, thread: ThreadId) {
        *self
            .views
            .lock()
            .expect("thread views poisoned")
            .watches
            .entry(thread)
            .or_default() += 1;
    }

    pub(super) fn is_watched(&self, thread: ThreadId) -> bool {
        self.views
            .lock()
            .expect("thread views poisoned")
            .watches
            .contains_key(&thread)
    }

    pub(super) fn git_activity(&self) {
        let _ = self.changes.send(Change::GitActivity);
    }
}

impl Workspace {
    /// Current monotonic workspace revision.
    ///
    /// Snapshot queries (`threads.list`, `projects.list`, `tasks.list`) read
    /// this BEFORE querying SQLite so any mutation committing concurrently
    /// emits a delta with `rev > snapshot.rev` that clients replay
    /// idempotently rather than skipping.
    pub(crate) fn revision(&self) -> u64 {
        self.updates.rev.load(Ordering::Acquire)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.updates.changes.subscribe()
    }

    pub(crate) fn emit_entity(&self, change: EntityChange) {
        let base_rev = self.updates.rev.fetch_add(1, Ordering::AcqRel);
        let _ = self.updates.changes.send(Change::Entity {
            base_rev,
            rev: base_rev + 1,
            change,
        });
    }

    pub(super) fn live_delta(&self, thread: ThreadId, delta: Arc<LiveDelta>) {
        let _ = self.updates.changes.send(Change::Live { thread, delta });
    }

    pub(super) fn changed(&self, thread: Option<ThreadId>) {
        if let Some(thread) = thread {
            let _ = self.updates.changes.send(Change::Thread(thread));
        } else {
            self.emit_entity(EntityChange::Resync);
        }
    }

    pub(super) fn harnesses_changed(&self) {
        let _ = self.updates.changes.send(Change::Harnesses);
    }

    pub(super) fn relist(&self, thread: ThreadId) {
        let _ = self.updates.changes.send(Change::Thread(thread));
        self.emit_entity(EntityChange::ThreadUpsert(thread));
    }

    pub(super) fn settled(&self, thread: ThreadId) {
        let _ = self.updates.changes.send(Change::Settled(thread));
        self.emit_entity(EntityChange::ThreadUpsert(thread));
    }

    pub(super) fn thread_removed(&self, thread: ThreadId) {
        let _ = self.updates.changes.send(Change::Settled(thread));
        self.emit_entity(EntityChange::ThreadRemoved(thread));
    }

    pub(super) fn project_changed(&self, path: String) {
        self.emit_entity(EntityChange::ProjectUpsert(path));
    }

    pub(super) fn project_removed(&self, path: String) {
        self.emit_entity(EntityChange::ProjectRemoved(path));
    }

    pub(crate) fn task_upserted(&self, task: TaskId, parent: Option<ThreadId>) {
        self.emit_entity(EntityChange::TaskUpsert(task));
        if let Some(parent) = parent {
            self.emit_entity(EntityChange::ThreadUpsert(parent));
        }
    }

    pub(crate) fn task_removed(&self, task: TaskId, parent: Option<ThreadId>) {
        self.emit_entity(EntityChange::TaskRemoved(task));
        if let Some(parent) = parent {
            self.emit_entity(EntityChange::ThreadUpsert(parent));
        }
    }

    pub(crate) fn workspace_resync(&self) {
        self.emit_entity(EntityChange::Resync);
    }

    pub(crate) async fn watched(&self, thread: ThreadId, mode: WatchChange) -> Result<(), Error> {
        match mode {
            WatchChange::Open => {
                let updates = Arc::clone(&self.updates);
                let cleared = self
                    .db
                    .write(move |conn| {
                        store::require(conn, thread)?;
                        let cleared = exec(
                            conn,
                            "UPDATE threads SET unread = 0 WHERE id = ?1 AND unread = 1",
                            [thread.into()],
                        )?;
                        updates.open_watch(thread);
                        Ok(cleared)
                    })
                    .await?;
                self.pull_due.notify_one();
                if cleared != 0 {
                    self.relist(thread);
                }
            }
            WatchChange::Close => {
                let mut views = self.updates.views.lock().expect("thread views poisoned");
                if let Some(watchers) = views.watches.get_mut(&thread) {
                    *watchers -= 1;
                    if *watchers == 0 {
                        views.watches.remove(&thread);
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn is_watched(&self, thread: ThreadId) -> bool {
        self.updates.is_watched(thread)
    }
}
