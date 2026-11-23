//! Side requests: attachments, transcript search, notifications, and older
//! history.

use std::sync::Arc;

use rune::AppHandle;
use viz_client::{
    Client, Result,
    protocol::{EntryId, ListEntries, SearchThreads, ThreadId},
};

use super::Session;
use crate::{app::Desktop, attachments::Source, model::Transcript as Thread};

impl Session<'_> {
    pub(super) async fn load_earlier(&mut self, thread: ThreadId, before: EntryId) -> Result<()> {
        if self.watched != Some(thread) || self.before != Some(before) {
            self.ui
                .update(move |s, tx| s.loaded_earlier(thread, None, tx));
            return Ok(());
        }
        match self
            .client
            .list_entries(&ListEntries {
                thread,
                before: Some(before),
                limit: None,
            })
            .await
        {
            Ok(page) => {
                self.before = page.before;
                self.entries.insert(0, Arc::new(page.entries));
                let history = Thread::from_entries(
                    self.entries.iter().flat_map(|batch| batch.iter()),
                    self.before,
                );
                self.ui
                    .update(move |s, tx| s.loaded_earlier(thread, Some(history), tx));
                Ok(())
            }
            Err(error) => {
                self.ui
                    .update(move |s, tx| s.loaded_earlier(thread, None, tx));
                Err(error)
            }
        }
    }
}

pub(super) fn search(ui: &AppHandle<Desktop>, client: &Client, request: u64, query: String) {
    let ui = ui.clone();
    let client = client.clone();
    tokio::spawn(async move {
        let result = client
            .search_threads(&SearchThreads {
                query: &query,
                limit: Some(24),
            })
            .await
            .map_err(|error| error.to_string());
        ui.update(move |s, _| s.palette.set_hits(request, result));
    });
}

/// Shows a desktop notification; a missing notification service is not an error
/// worth a toast. The freedesktop backend talks D-Bus on this runtime;
/// elsewhere `show` blocks, so it runs on the blocking pool.
pub(super) fn notify(title: &str, body: &'static str) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("viz").summary(title).body(body);
    #[cfg(all(unix, not(target_os = "macos")))]
    tokio::spawn(async move {
        _ = notification.show_async().await;
    });
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    tokio::task::spawn_blocking(move || {
        _ = notification.show();
    });
}

pub(super) fn attach(
    ui: &AppHandle<Desktop>,
    scope: u64,
    target: crate::attachments::Target,
    source: Source,
) {
    let ui = ui.clone();
    tokio::task::spawn_blocking(move || {
        let loaded = source.load();
        ui.update(move |s, tx| s.loaded_attachments(scope, target, loaded, tx));
    });
}
