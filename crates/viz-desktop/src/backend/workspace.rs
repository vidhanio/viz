//! Transport for the workspace inspector, attachments, search, and older
//! history.

use std::mem;

use rune::{AppHandle, Tx};
use serde_json::Value;
use viz_client::{
    Client, Result, delta,
    protocol::{
        CommitResult, ConversationId, EntryId, EntryPage, GitCheckout, GitCommit, GitDiff, GitPr,
        GitRevert, ListEntries, SearchConversations,
    },
};

use super::{Session, toast};
use crate::{
    app::Desktop,
    attachments::Source,
    changes::{self, Request},
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

pub(super) fn attach(ui: &AppHandle<Desktop>, conversation: ConversationId, source: Source) {
    let ui = ui.clone();
    tokio::task::spawn_blocking(move || {
        let loaded = source.load();
        ui.update(move |s, tx| s.loaded_attachments(conversation, loaded, tx));
    });
}

pub(super) fn git(ui: &AppHandle<Desktop>, client: &Client, request: Request) {
    let ui = ui.clone();
    let client = client.clone();
    tokio::spawn(async move {
        if let Err(error) = git_request(&ui, &client, request).await {
            toast(&ui, "Git request failed", error.to_string());
        }
    });
}

pub(super) fn git_offline(ui: &AppHandle<Desktop>, request: &Request) {
    match *request {
        Request::Diff {
            conversation,
            request,
            ..
        } => selected_update(ui, conversation, move |s, _| {
            s.changes.set_diff(request, Err("Offline".into()));
        }),
        Request::Branches(conversation) => selected_update(ui, conversation, |s, _| {
            s.changes.set_branches(Err("Offline".into()));
        }),
        Request::Commit { conversation, .. } => selected_update(ui, conversation, |s, tx| {
            changes::commit_result(s, Err("Backend disconnected".into()), tx);
        }),
        _ => toast(ui, "Offline", "Backend disconnected".into()),
    }
}

async fn git_request(ui: &AppHandle<Desktop>, client: &Client, request: Request) -> Result<()> {
    match request {
        Request::Diff {
            conversation,
            scope,
            turn,
            request,
        } => {
            let result = client
                .git_diff(&GitDiff {
                    conversation,
                    scope,
                    turn,
                    path: None,
                })
                .await
                .map_err(|error| error.to_string());
            selected_update(ui, conversation, move |s, _| {
                s.changes.set_diff(request, result);
            });
        }
        Request::Branches(conversation) => {
            let result = client
                .git_branches(conversation)
                .await
                .map_err(|error| error.to_string());
            selected_update(ui, conversation, move |s, _| s.changes.set_branches(result));
        }
        Request::Checkout {
            conversation,
            branch,
            create,
        } => {
            client
                .git_checkout(&GitCheckout {
                    conversation,
                    branch: &branch,
                    create: Some(create),
                })
                .await?;
            selected_update(ui, conversation, changes::refresh);
        }
        Request::Commit {
            conversation,
            message,
        } => {
            let result = client
                .git_commit(&GitCommit {
                    conversation,
                    message: message.as_deref(),
                    all: None,
                })
                .await
                .map_err(|error| error.to_string());
            committed(ui, conversation, result);
        }
        Request::Push(conversation) => {
            client.git_push(conversation).await?;
            ui.update(|s, tx| s.toast(tx, "Pushed", None));
            selected_update(ui, conversation, changes::refresh);
        }
        Request::Pr {
            conversation,
            title,
            body,
            draft,
        } => {
            let result = client
                .git_pr(&GitPr {
                    conversation,
                    title: title.as_deref(),
                    body: body.as_deref(),
                    draft: Some(draft),
                })
                .await?;
            ui.update(move |s, tx| {
                if s.selected == Some(conversation) {
                    changes::pr_result(s, result.url, tx);
                    changes::refresh(s, tx);
                } else {
                    s.toast(tx, "Pull request created", Some(result.url));
                }
            });
        }
        Request::Revert { conversation, turn } => {
            client.git_revert(&GitRevert { conversation, turn }).await?;
            ui.update(|s, tx| s.toast(tx, "Working tree restored", None));
            selected_update(ui, conversation, changes::refresh);
        }
    }
    Ok(())
}

/// Shows a commit's outcome in the dialog that asked for it, or as a toast once
/// the user moved on.
fn committed(
    ui: &AppHandle<Desktop>,
    conversation: ConversationId,
    result: std::result::Result<CommitResult, String>,
) {
    ui.update(move |s, tx| {
        if s.selected != Some(conversation) {
            match result {
                Ok(result) => s.toast(tx, "Committed", Some(result.message)),
                Err(error) => s.toast(tx, "Commit failed", Some(error)),
            }
            return;
        }
        let ok = result.is_ok();
        changes::commit_result(s, result, tx);
        if ok {
            changes::refresh(s, tx);
        }
    });
}

fn selected_update(
    ui: &AppHandle<Desktop>,
    conversation: ConversationId,
    f: impl FnOnce(&mut Desktop, &mut Tx) + Send + 'static,
) {
    ui.update(move |s, tx| {
        if s.selected == Some(conversation) {
            f(s, tx);
        }
    });
}
