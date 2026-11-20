//! Side requests: attachments, transcript search, notifications, and older
//! history.

use std::mem;

use rune::AppHandle;
use serde_json::Value;
use viz_client::{
    Client, Result, delta,
    protocol::{ConversationId, EntryId, EntryPage, ListEntries, SearchConversations},
};

use super::Session;
use crate::{
    app::Desktop,
    attachments::Source,
    thread::{self, Thread},
};

pub(super) struct History {
    entries: Vec<Value>,
    before: Option<EntryId>,
}

impl History {
    fn prepend(&mut self, mut page: EntryPage) {
        self.before = page.before;
        page.entries.append(&mut self.entries);
        self.entries = page.entries;
    }

    pub(super) fn thread(&self, view: &Value) -> Thread {
        Thread::from_history(view, &self.entries, self.before)
    }

    pub(super) fn apply(
        &mut self,
        view: &mut Value,
        ops: &[Value],
    ) -> std::result::Result<(), delta::ApplyError> {
        for op in ops {
            if let Some(count) = window_trim(op)
                && let Some(entries) = view.get_mut("entries").and_then(Value::as_array_mut)
                && let Some(dropped) = entries.get_mut(..count)
            {
                // Retain the prefix the watched window is about to drop, once
                // the user chose to keep earlier history in this viewport.
                self.entries.extend(dropped.iter_mut().map(mem::take));
            }
            delta::apply(view, std::slice::from_ref(op))?;
        }
        Ok(())
    }
}

fn window_trim(op: &Value) -> Option<usize> {
    let [verb, path, index, count, inserted] = op.as_array()?.as_slice() else {
        return None;
    };
    let [segment] = path.as_array()?.as_slice() else {
        return None;
    };
    if verb.as_str() != Some("p")
        || segment.as_str() != Some("entries")
        || index.as_u64() != Some(0)
        || !inserted.as_array()?.is_empty()
    {
        return None;
    }
    usize::try_from(count.as_u64()?).ok()
}

impl Session<'_> {
    pub(super) async fn load_earlier(
        &mut self,
        conversation: ConversationId,
        before: EntryId,
    ) -> Result<()> {
        let thread = self.older(conversation, before).await;
        match thread {
            Ok(thread) => {
                self.ui
                    .update(move |s, tx| s.loaded_earlier(conversation, thread, tx));
                Ok(())
            }
            Err(error) => {
                self.ui
                    .update(move |s, tx| s.loaded_earlier(conversation, None, tx));
                Err(error)
            }
        }
    }

    async fn older(
        &mut self,
        conversation: ConversationId,
        before: EntryId,
    ) -> Result<Option<Thread>> {
        let Some(view) = self.view.as_ref().filter(|_| self.watched == conversation) else {
            return Ok(None);
        };
        let actual = self
            .history
            .as_ref()
            .map_or_else(|| thread::before_cursor(view), |history| history.before);
        if actual != Some(before) {
            return Ok(None);
        }
        let page = self
            .client
            .list_entries(&ListEntries {
                conversation,
                before,
                limit: None,
            })
            .await?;
        let history = self.history.get_or_insert_with(|| History {
            entries: Vec::new(),
            before: None,
        });
        history.prepend(page);
        Ok(Some(history.thread(view)))
    }
}

pub(super) fn search(ui: &AppHandle<Desktop>, client: &Client, request: u64, query: String) {
    let ui = ui.clone();
    let client = client.clone();
    tokio::spawn(async move {
        let result = client
            .search_conversations(&SearchConversations {
                query: &query,
                limit: Some(24),
            })
            .await
            .map_err(|error| error.to_string());
        ui.update(move |s, _| s.palette.set_hits(request, result));
    });
}

/// Shows a desktop notification; a missing notification service is not an error
/// worth a toast.
pub(super) fn notify(title: &str, body: &'static str) {
    let mut notification = notify_rust::Notification::new();
    notification.appname("viz").summary(title).body(body);
    // The freedesktop backend talks D-Bus on this runtime; elsewhere `show`
    // blocks.
    #[cfg(all(unix, not(target_os = "macos")))]
    tokio::spawn(async move {
        _ = notification.show_async().await;
    });
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    tokio::task::spawn_blocking(move || {
        _ = notification.show();
    });
}

pub(super) fn attach(ui: &AppHandle<Desktop>, target: crate::attachments::Target, source: Source) {
    let ui = ui.clone();
    tokio::task::spawn_blocking(move || {
        let loaded = source.load();
        ui.update(move |s, tx| s.loaded_attachments(target, loaded, tx));
    });
}
