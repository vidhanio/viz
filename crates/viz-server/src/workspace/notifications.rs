//! Thread watches and live transcript projections. Durable changes are
//! published after commit. Watch and live state updates that must agree with
//! persisted rows run on the SQLite writer thread immediately after commit.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::broadcast;

use super::{Workspace, sql::exec, store};
use crate::{
    error::Error,
    wire::{LiveTurn, ThreadId},
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    Thread(ThreadId),
    Settled(ThreadId),
    Directory,
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
    live: HashMap<ThreadId, LiveTurn>,
    watches: HashMap<ThreadId, usize>,
}

pub(super) struct Updates {
    changes: broadcast::Sender<Change>,
    views: Mutex<Views>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            changes: broadcast::channel(1024).0,
            views: Mutex::default(),
        }
    }
}

impl Updates {
    pub(super) fn live(&self, thread: ThreadId, live: Option<LiveTurn>) {
        let mut views = self.views.lock().expect("thread views poisoned");
        if let Some(live) = live {
            views.live.insert(thread, live);
        } else {
            views.live.remove(&thread);
        }
    }

    pub(super) fn latest(&self, thread: ThreadId) -> Option<LiveTurn> {
        self.raw_live(thread).map(store::live_preview)
    }

    pub(super) fn raw_live(&self, thread: ThreadId) -> Option<LiveTurn> {
        self.views
            .lock()
            .expect("thread views poisoned")
            .live
            .get(&thread)
            .cloned()
    }

    pub(super) fn remove(&self, thread: ThreadId) {
        let mut views = self.views.lock().expect("thread views poisoned");
        views.live.remove(&thread);
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
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.updates.changes.subscribe()
    }

    pub(super) fn changed(&self, thread: Option<ThreadId>) {
        let _ = self
            .updates
            .changes
            .send(thread.map_or(Change::Directory, Change::Thread));
    }

    pub(super) fn harnesses_changed(&self) {
        let _ = self.updates.changes.send(Change::Harnesses);
    }

    pub(crate) fn directory_changed(&self) {
        self.changed(None);
    }

    pub(super) fn relist(&self, thread: ThreadId) {
        self.changed(Some(thread));
        self.changed(None);
    }

    pub(super) fn settled(&self, thread: ThreadId) {
        let _ = self.updates.changes.send(Change::Settled(thread));
        self.changed(None);
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
