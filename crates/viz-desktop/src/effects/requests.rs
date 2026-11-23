//! Asynchronous RPC request helpers dispatched by [`super::server::Session`].

use std::sync::Arc;

use rune::AppHandle;
use viz_client::{
    Client, Error, Result,
    protocol::{
        CreateProject, EntryId, HarnessOptions, LaunchThread, ListBranches, ListEntries,
        ListWorktrees, Position, RemoveProject, SearchFiles, SearchThreads, SendInput, ThreadId,
        TriageAction, TriageThread, UpdateProject, UpdateThread,
    },
};

use super::{ProjectChange, ThreadChange, server::Session};
use crate::{
    app::Desktop,
    model::{Checkouts, Transcript},
    update::{Msg, ServerMsg},
};

impl Session<'_> {
    pub(super) async fn launch(&self, draft: u64, params: LaunchThread) -> Result<()> {
        match self.client.launch_thread(&params).await {
            Ok(thread) => self.dispatch(ServerMsg::ThreadCreated { draft, thread }),
            Err(error) => {
                let detail = error.to_string();
                let closed = matches!(error, Error::Closed);
                self.dispatch(ServerMsg::LaunchFailed {
                    draft,
                    params: Box::new(params),
                    title: "Request failed".into(),
                    detail: Some(detail),
                });
                if closed {
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn send_input(&self, input: SendInput) -> Result<()> {
        match self.client.send(&input).await {
            Ok(_) => Ok(()),
            Err(error) => {
                let detail = error.to_string();
                let closed = matches!(error, Error::Closed);
                self.dispatch(ServerMsg::SendFailed {
                    input: Box::new(input),
                    title: "Request failed".into(),
                    detail: Some(detail),
                });
                if closed { Err(error) } else { Ok(()) }
            }
        }
    }

    pub(super) fn load_draft(&self, draft: u64) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .list_harnesses()
                .await
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::DraftHarnessesLoaded {
                        draft,
                        result: Some(result),
                    }),
                    tx,
                );
            });
        });
    }

    pub(super) async fn update_thread(
        &self,
        op_id: u64,
        thread: ThreadId,
        change: &ThreadChange,
    ) -> Result<()> {
        let result = self.run_thread_change(thread, change).await;
        match result {
            Ok(()) => {
                self.dispatch(ServerMsg::MutationOk { op_id });
                Ok(())
            }
            Err(error) => {
                let detail = error.to_string();
                let closed = matches!(error, Error::Closed);
                self.dispatch(ServerMsg::MutationFailed {
                    op_id,
                    title: "Request failed".into(),
                    detail,
                });
                if closed { Err(error) } else { Ok(()) }
            }
        }
    }

    async fn run_thread_change(&self, thread: ThreadId, change: &ThreadChange) -> Result<()> {
        let mut params = UpdateThread {
            thread,
            title: None,
            project: None,
        };
        let triage = async |action| {
            let request = TriageThread { thread, action };
            self.client.request(&request).await.map(drop)
        };
        match change {
            ThreadChange::Title(title) => params.title = Some(title),
            ThreadChange::Archive(true) => return triage(TriageAction::Archive).await,
            ThreadChange::Archive(false) => return triage(TriageAction::Unarchive).await,
            ThreadChange::Pin(true) => {
                let position = Position::Top;
                return triage(TriageAction::Pin { position }).await;
            }
            ThreadChange::Pin(false) => return triage(TriageAction::Unpin).await,
            ThreadChange::Unread(true) => return triage(TriageAction::MarkUnread).await,
            ThreadChange::Unread(false) => return triage(TriageAction::MarkRead).await,
            ThreadChange::Project(project) => params.project = Some(project.as_deref()),
        }
        self.client.update_thread(&params).await
    }

    pub(super) async fn project(&self, op_id: Option<u64>, change: ProjectChange) -> Result<()> {
        let result = match &change {
            ProjectChange::Add(path) => {
                let project = self
                    .client
                    .create_project(&CreateProject { path, name: None })
                    .await;
                match project {
                    Ok(project) => {
                        self.dispatch(ServerMsg::ProjectAdded(Box::new(project)));
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            ProjectChange::Rename { id, name } => {
                self.client
                    .update_project(&UpdateProject {
                        changes: None,
                        project: id,
                        name: Some(name),
                    })
                    .await
            }
            ProjectChange::Remove(id) => {
                self.client
                    .remove_project(&RemoveProject { project: id })
                    .await
            }
        };
        match (op_id, result) {
            (Some(op_id), Ok(())) => {
                self.dispatch(ServerMsg::MutationOk { op_id });
                Ok(())
            }
            (Some(op_id), Err(error)) => {
                let detail = error.to_string();
                let closed = matches!(error, Error::Closed);
                self.dispatch(ServerMsg::MutationFailed {
                    op_id,
                    title: "Request failed".into(),
                    detail,
                });
                if closed { Err(error) } else { Ok(()) }
            }
            (None, other) => other,
        }
    }

    pub(super) async fn load_earlier(&mut self, thread: ThreadId, before: EntryId) -> Result<()> {
        if self.watched != Some(thread) || self.before != Some(before) {
            self.dispatch(ServerMsg::EarlierLoaded {
                thread,
                transcript: None,
            });
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
                let transcript = Transcript::from_entries(
                    self.entries.iter().flat_map(|batch| batch.iter()),
                    self.before,
                );
                self.dispatch(ServerMsg::EarlierLoaded {
                    thread,
                    transcript: Some(transcript),
                });
                Ok(())
            }
            Err(error) => {
                self.dispatch(ServerMsg::EarlierLoaded {
                    thread,
                    transcript: None,
                });
                Err(error)
            }
        }
    }

    pub(super) fn search_files(&self, thread: ThreadId, request: u64, query: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .search_files(&SearchFiles {
                    thread,
                    query: &query,
                    limit: Some(24),
                })
                .await
                .map(|matches| matches.into_iter().map(|found| found.path).collect())
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::FileSearchResult {
                        thread,
                        request,
                        result,
                    }),
                    tx,
                );
            });
        });
    }

    pub(super) fn load_checkouts(&self, project: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        let environment = self.environment.to_string();
        tokio::spawn(async move {
            let result = async {
                let mut branches = Vec::new();
                let mut offset = None;
                loop {
                    let page = client
                        .request(&ListBranches {
                            project: &project,
                            query: None,
                            offset,
                            limit: None,
                        })
                        .await?;
                    branches.extend(page.branches);
                    offset = page.next;
                    if offset.is_none() {
                        break;
                    }
                }
                let worktrees = client.request(&ListWorktrees { project: &project }).await?;
                Ok::<_, Error>(Checkouts {
                    branches,
                    worktrees: worktrees.worktrees,
                })
            }
            .await
            .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::CheckoutsLoaded {
                        environment,
                        project,
                        result,
                    }),
                    tx,
                );
            });
        });
    }

    pub(super) fn harness_options(&self, harness: String, model: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        let environment = self.environment.to_string();
        tokio::spawn(async move {
            let result = client
                .harness_options(&HarnessOptions {
                    harness: harness.clone(),
                    model,
                })
                .await
                .map(|reply| reply.options)
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::HarnessOptionsLoaded {
                        environment,
                        harness,
                        result,
                    }),
                    tx,
                );
            });
        });
    }
}

/// Runs a transcript full-text search for the command palette.
pub(super) fn search_threads(
    ui: &AppHandle<Desktop>,
    client: &Client,
    request: u64,
    query: String,
) {
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
        ui.update(move |s, tx| {
            s.dispatch(
                Msg::Server(ServerMsg::ThreadSearchResult { request, result }),
                tx,
            );
        });
    });
}
