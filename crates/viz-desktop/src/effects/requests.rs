//! Asynchronous RPC request helpers dispatched by [`super::server::Session`].

use std::sync::Arc;

use base64::Engine as _;
use rune::AppHandle;
use viz_client::{
    Client, Error, Result,
    protocol::{
        EntryId, HarnessOptions, LaunchThread, ListBranches, ListEntries, ListTasks, ListWorktrees,
        ProjectIconResult, ProjectSettingsPatch, ReadProjectIcon, RetitleThread, SearchThreads,
        SendInput, ThreadId, TriageThread, UpdateProject, UpdateThread,
    },
};

use super::{ProjectChange, ThreadChange, server::Session};
use crate::{
    app::Desktop,
    model::{Checkouts, Transcript},
    update::{Msg, ServerMsg},
};

impl Session<'_> {
    /// Relists harnesses so the model picker's catalog stays current.
    pub(super) async fn refresh_harnesses(&self) -> Result<()> {
        let harnesses = self.client.list_harnesses().await?;
        self.dispatch(ServerMsg::HarnessesSynced(harnesses));
        Ok(())
    }

    /// `thread.access`: applies or clears a thread's access mode.
    pub(super) async fn set_access(
        &self,
        thread: ThreadId,
        access: Option<viz_client::protocol::AccessMode>,
    ) -> Result<()> {
        self.client
            .request(&viz_client::protocol::SetThreadAccess { thread, access })
            .await
            .map(drop)
    }

    pub(super) async fn launch(&self, draft: u64, params: Box<LaunchThread>) -> Result<()> {
        match self.client.launch_thread(&params).await {
            Ok(thread) => self.dispatch(ServerMsg::ThreadCreated { draft, thread }),
            Err(error) => {
                let detail = error.to_string();
                let closed = matches!(error, Error::Closed);
                self.dispatch(ServerMsg::LaunchFailed {
                    draft,
                    params,
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
        match change {
            ThreadChange::Triage(action) => {
                let request = TriageThread {
                    thread,
                    action: action.clone(),
                };
                match self.client.request(&request).await {
                    Ok(triaged) => {
                        self.dispatch(ServerMsg::ThreadTriaged {
                            op_id,
                            thread,
                            lifecycle: Box::new(triaged.lifecycle),
                        });
                        Ok(())
                    }
                    Err(error) => self.fail_mutation(op_id, error),
                }
            }
            ThreadChange::Title(title) => {
                let params = UpdateThread {
                    thread,
                    title: Some(title),
                    project: None,
                };
                match self.client.update_thread(&params).await {
                    Ok(()) => {
                        self.dispatch(ServerMsg::MutationOk { op_id });
                        Ok(())
                    }
                    Err(error) => self.fail_mutation(op_id, error),
                }
            }
            ThreadChange::Project(project) => {
                let params = UpdateThread {
                    thread,
                    title: None,
                    project: Some(project.as_deref()),
                };
                match self.client.update_thread(&params).await {
                    Ok(()) => {
                        self.dispatch(ServerMsg::MutationOk { op_id });
                        Ok(())
                    }
                    Err(error) => self.fail_mutation(op_id, error),
                }
            }
        }
    }

    fn fail_mutation(&self, op_id: u64, error: Error) -> Result<()> {
        let detail = error.to_string();
        let closed = matches!(error, Error::Closed);
        self.dispatch(ServerMsg::MutationFailed {
            op_id,
            title: "Request failed".into(),
            detail,
        });
        if closed { Err(error) } else { Ok(()) }
    }

    pub(super) async fn retitle_thread(&self, thread: ThreadId) -> Result<()> {
        let updated = self.client.request(&RetitleThread { thread }).await?;
        self.dispatch(ServerMsg::ThreadRetitled {
            thread,
            title: updated.title,
        });
        Ok(())
    }

    pub(super) fn read_project_icon(&self, project: String, etag: Option<String>) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        let environment = self.environment.to_string();
        tokio::spawn(async move {
            let Ok(reply) = client
                .project_icon(&ReadProjectIcon {
                    project: project.clone(),
                    etag,
                })
                .await
            else {
                return;
            };
            let payload = match reply {
                ProjectIconResult::None => Some((None, None)),
                ProjectIconResult::Unchanged { .. } => None,
                ProjectIconResult::Image {
                    etag,
                    media_type,
                    data,
                } => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok();
                    Some((Some(etag), bytes.map(|bytes| (media_type, bytes))))
                }
            };
            if let Some((etag, image)) = payload {
                ui.update(move |s, tx| {
                    s.dispatch(
                        Msg::Server(ServerMsg::ProjectIconLoaded {
                            environment,
                            project,
                            etag,
                            image,
                        }),
                        tx,
                    );
                });
            }
        });
    }

    pub(super) async fn project(&self, op_id: Option<u64>, change: ProjectChange) -> Result<()> {
        let result = match &change {
            ProjectChange::Pin { id, pinned } => {
                self.client
                    .update_project(&UpdateProject {
                        changes: Some(ProjectSettingsPatch {
                            pinned: Some(*pinned),
                            ..ProjectSettingsPatch::default()
                        }),
                        project: id,
                        name: None,
                    })
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
                    model: model.clone(),
                })
                .await
                .map(|reply| reply.options)
                .map_err(|error| error.to_string());
            ui.update(move |s, tx| {
                s.dispatch(
                    Msg::Server(ServerMsg::HarnessOptionsLoaded {
                        environment,
                        harness,
                        model,
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

impl Session<'_> {
    pub(super) async fn answer(
        &self,
        answer: viz_client::protocol::AnswerRequest,
    ) -> viz_client::Result<()> {
        let result = self.client.request(&answer).await;
        let closed = matches!(result, Err(viz_client::Error::Closed));
        let environment = Arc::clone(&self.environment);
        self.ui.update(move |app, tx| {
            if app.model.active_env_key() != environment.as_ref() {
                return;
            }
            app.dispatch(
                Msg::Requests(crate::update::requests::RequestMsg::Finished(
                    answer.request,
                    result.map(drop).map_err(|error| error.to_string()),
                )),
                tx,
            );
        });
        if closed {
            return Err(viz_client::Error::Closed);
        }
        Ok(())
    }

    pub(super) async fn refresh_tasks(&self) -> Result<()> {
        let tasks = self.client.request(&ListTasks::default()).await?.tasks;
        self.dispatch(ServerMsg::TasksSynced {
            environment: self.environment.to_string(),
            tasks,
        });
        Ok(())
    }

    pub(super) async fn refresh_list(&self) -> Result<()> {
        let (threads, mut projects, ()) = tokio::try_join!(
            self.client.list_threads(),
            self.client.list_projects(),
            self.refresh_tasks(),
        )?;
        projects.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        self.dispatch(ServerMsg::WorkspaceSynced {
            environment: self.environment.to_string(),
            projects,
            threads,
        });
        Ok(())
    }
}
