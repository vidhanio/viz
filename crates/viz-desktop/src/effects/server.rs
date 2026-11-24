//! Background server connection runtime: connects (retrying while offline),
//! watches the selected thread, and feeds typed [`crate::update::ServerMsg`]
//! events back into the UI.

use std::{sync::Arc, time::Duration};

use rune::AppHandle;
use tokio::{sync::mpsc, task::JoinSet};
use viz_client::{
    Client, Connection, Endpoint, Error,
    protocol::{
        CancelTask, ClientInfo, ClientKind, ConfigureThread, DeleteThread, ListEnvironments,
        MoveThread, Notification, ThreadId,
    },
};

use super::{ServerEffect, git, offline, platform, requests};
use crate::{
    app::Desktop,
    model::Status,
    update::{Msg, ServerMsg},
};

/// Delay between reconnect attempts while disconnected.
const RETRY: Duration = Duration::from_secs(3);
const NOT_FOUND: i64 = -32001;

/// Starts the background Tokio thread owning the server WebSocket session.
/// `first` fires once the first connection attempt has reached the UI queue.
pub fn spawn(
    ui: AppHandle<Desktop>,
    commands: mpsc::UnboundedReceiver<ServerEffect>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run(&ui, commands, first, endpoint)),
            Err(error) => {
                tracing::error!(%error, "could not start the connection runtime");
                dispatch(
                    &ui,
                    ServerMsg::StatusChanged(Status::Error(format!(
                        "Couldn’t start the connection thread: {error}"
                    ))),
                );
                _ = first.send(());
            }
        }
    });
}

fn dispatch(ui: &AppHandle<Desktop>, msg: ServerMsg) {
    ui.update(move |s, tx| s.dispatch(Msg::Server(msg), tx));
}

pub(super) fn dispatch_ui(ui: &AppHandle<Desktop>, msg: ServerMsg) {
    dispatch(ui, msg);
}

pub(super) fn toast(ui: &AppHandle<Desktop>, title: &str, detail: String) {
    dispatch(
        ui,
        ServerMsg::RequestFailed {
            title: title.to_owned(),
            detail,
        },
    );
}

fn signal(first: &mut Option<std::sync::mpsc::Sender<()>>) {
    if let Some(first) = first.take() {
        _ = first.send(());
    }
}

async fn run(
    ui: &AppHandle<Desktop>,
    mut commands: mpsc::UnboundedReceiver<ServerEffect>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    let mut first = Some(first);
    let endpoint = match endpoint {
        Ok(endpoint) => endpoint,
        Err(detail) => {
            dispatch(ui, ServerMsg::StatusChanged(Status::Error(detail.clone())));
            signal(&mut first);
            while let Some(command) = commands.recv().await {
                if !offline::handle(ui, command, &mut None, &mut None) {
                    dispatch(ui, ServerMsg::StatusChanged(Status::Error(detail.clone())));
                }
            }
            return;
        }
    };
    dispatch(ui, ServerMsg::Endpoint(endpoint.to_string()));
    let mut selected = None;
    let mut environment = None;
    loop {
        let info = ClientInfo {
            kind: ClientKind::App,
            name: "desktop".into(),
        };
        let lost = match Client::connect(&endpoint, info, environment.clone()).await {
            Ok(connection) => {
                match session(ui, connection, &mut commands, &mut selected, &mut first).await {
                    Ended::Quit => return,
                    Ended::Environment { target, thread } => {
                        selected = thread;
                        environment = Some(target);
                        continue;
                    }
                    Ended::Lost(error) => {
                        toast(ui, "Lost the connection to the server", error.to_string());
                        error.to_string()
                    }
                }
            }
            Err(error) => error.to_string(),
        };
        tracing::debug!(%endpoint, "server connection unavailable; retrying");
        dispatch(ui, ServerMsg::StatusChanged(Status::Error(lost)));
        signal(&mut first);
        let retry = tokio::time::sleep(RETRY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = &mut retry => break,
                command = commands.recv() => match command {
                    Some(command) => if !offline::handle(ui, command, &mut selected, &mut environment) {
                        break;
                    },
                    None => return,
                },
            }
        }
    }
}

/// Why a live connection session ended.
enum Ended {
    Quit,
    Lost(Error),
    Environment {
        target: String,
        thread: Option<ThreadId>,
    },
}

/// One live connection: sync lists, watch the selected thread, and process
/// commands and push notifications until closed or switched.
async fn session(
    ui: &AppHandle<Desktop>,
    connection: Connection,
    commands: &mut mpsc::UnboundedReceiver<ServerEffect>,
    selected: &mut Option<ThreadId>,
    first: &mut Option<std::sync::mpsc::Sender<()>>,
) -> Ended {
    let Connection {
        client,
        mut notifications,
        hello,
    } = connection;
    let mut session = Session {
        ui,
        client,
        environment: Arc::from(hello.environment.as_str()),
        watched: None,
        git_watched: None,
    };
    if let Err(error) = session.start(hello, selected).await {
        return Ended::Lost(error);
    }
    signal(first);
    let mut jobs = JoinSet::new();
    let mut configs: std::collections::VecDeque<(
        ThreadId,
        String,
        viz_client::protocol::OptionValue,
    )> = std::collections::VecDeque::new();
    let mut configuring = false;
    loop {
        if !configuring && let Some((thread, option, value)) = configs.pop_front() {
            configuring = true;
            let client = session.client.clone();
            jobs.spawn(async move {
                let result = client
                    .configure_thread(&ConfigureThread {
                        thread,
                        option,
                        value,
                    })
                    .await;
                BackgroundReply::Configured(result)
            });
        }
        let result = tokio::select! {
            notification = notifications.recv() => match notification {
                Some(notification) => session.on_notification(notification).await,
                None => Err(Error::Closed),
            },
            done = jobs.join_next(), if !jobs.is_empty() => match done {
                Some(Ok(BackgroundReply::Configured(result))) => {
                    configuring = false;
                    result
                }
                Some(Ok(BackgroundReply::Git { reply, refresh })) => {
                    session.dispatch(reply);
                    if refresh { session.refresh_list().await } else { Ok(()) }
                }
                Some(Err(error)) => {
                    tracing::error!(%error, "desktop request task ended unexpectedly");
                    return Ended::Lost(Error::Closed);
                }
                None => Ok(()),
            },
            command = commands.recv() => match command {
                Some(ServerEffect::Environment { environment, thread }) => {
                    session.client.close();
                    return Ended::Environment { target: environment, thread };
                }
                Some(ServerEffect::Git { request: git::Request::Watch(next), .. }) => session.watch_git(next).await,
                Some(ServerEffect::Git { thread, serial, request }) => {
                    if session.watched == Some(thread) {
                        let client = session.client.clone();
                        let refresh = matches!(&request, git::Request::Init(_) | git::Request::CreatePull { .. });
                        jobs.spawn(async move {
                            let reply = git::execute(client, thread, serial, request).await;
                            BackgroundReply::Git { reply, refresh }
                        });
                    }
                    Ok(())
                }
                Some(ServerEffect::Configure { thread, option, value }) => {
                    configs.push_back((thread, option, value));
                    Ok(())
                }
                Some(command) => session.on_command(command, selected).await,
                None => return Ended::Quit,
            },
        };
        match result {
            Ok(()) => {}
            Err(Error::Closed) => return Ended::Lost(Error::Closed),
            Err(error) => toast(ui, "Request failed", error.to_string()),
        }
    }
}

enum BackgroundReply {
    Configured(viz_client::Result<()>),
    Git { reply: ServerMsg, refresh: bool },
}

/// Active WebSocket session state owned by the connection task.
pub(super) struct Session<'a> {
    pub(super) ui: &'a AppHandle<Desktop>,
    pub(super) client: Client,
    pub(super) environment: Arc<str>,
    pub(super) watched: Option<ThreadId>,
    pub(super) git_watched: Option<ThreadId>,
}

impl Session<'_> {
    async fn start(
        &mut self,
        hello: viz_client::protocol::Hello,
        selected: &mut Option<ThreadId>,
    ) -> viz_client::Result<()> {
        let ((), harnesses, environments) = tokio::try_join!(
            self.refresh_list(),
            self.client.list_harnesses(),
            self.client.request(&ListEnvironments {}),
        )?;
        if let Some(thread) = *selected {
            match self.switch(Some(thread)).await {
                Err(Error::Rpc {
                    code: NOT_FOUND, ..
                }) => *selected = None,
                result => result?,
            }
        }
        self.dispatch(ServerMsg::Connected {
            version: hello.version,
            environment: hello.environment,
            environments: environments.environments,
            harnesses,
            watched: *selected,
        });
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "single exhaustive server effect dispatcher"
    )]
    async fn on_command(
        &mut self,
        command: ServerEffect,
        selected: &mut Option<ThreadId>,
    ) -> viz_client::Result<()> {
        match command {
            ServerEffect::Answer(answer) => self.answer(answer).await,
            ServerEffect::Select(thread) => {
                self.select(thread).await?;
                *selected = self.watched;
                Ok(())
            }
            ServerEffect::ClearSelection => {
                self.switch(None).await?;
                *selected = None;
                Ok(())
            }
            ServerEffect::Create {
                draft,
                params,
                recovery,
            } => self.launch(draft, *params, recovery).await,
            ServerEffect::CancelTask(task) => {
                self.client
                    .request(&CancelTask {
                        task: viz_client::protocol::TaskId(task),
                    })
                    .await?;
                self.refresh_tasks().await
            }
            ServerEffect::Environment { .. } => {
                unreachable!("environment changes replace the session")
            }
            ServerEffect::LoadDraft(id) => {
                self.load_draft(id);
                Ok(())
            }
            ServerEffect::Harnesses => self.refresh_harnesses().await,
            ServerEffect::Access { thread, access } => self.set_access(thread, access).await,
            ServerEffect::HarnessOptions { harness, model } => {
                self.harness_options(harness, model);
                Ok(())
            }
            ServerEffect::Update {
                op_id,
                thread,
                change,
            } => self.update_thread(op_id, thread, &change).await,
            ServerEffect::Retitle(thread) => self.retitle_thread(thread).await,
            ServerEffect::ReadProjectIcon { project, etag } => {
                self.read_project_icon(project, etag);
                Ok(())
            }
            ServerEffect::Delete {
                thread,
                remove_worktree,
            } => {
                self.client
                    .delete_thread(&DeleteThread {
                        thread,
                        remove_worktree,
                    })
                    .await
            }
            ServerEffect::LoadCheckouts(project) => {
                self.load_checkouts(project);
                Ok(())
            }
            ServerEffect::Move { thread, worktree } => self
                .client
                .request(&MoveThread { thread, worktree })
                .await
                .map(drop),
            ServerEffect::Abort(id) => self.client.stop_thread(id).await,
            ServerEffect::Project { op_id, change } => self.project(op_id, change).await,
            ServerEffect::SearchFiles {
                thread,
                project,
                request,
                query,
            } => {
                self.search_files(thread, project, request, query);
                Ok(())
            }
            ServerEffect::ProjectPalette(request, command) => {
                self.project_palette(request, command);
                Ok(())
            }
            ServerEffect::SearchGitHub {
                project,
                request,
                query,
            } => {
                self.search_github(project, request, query);
                Ok(())
            }
            ServerEffect::SteerQueued { thread, run } => self.steer_queued(thread, run).await,
            ServerEffect::UpdateQueued { thread, run, text } => {
                self.update_queued(thread, run, &text).await
            }
            ServerEffect::RemoveQueued { thread, run } => self.remove_queued(thread, run).await,
            ServerEffect::SearchThreads { request, query } => {
                requests::search_threads(self.ui, &self.client, request, query);
                Ok(())
            }
            ServerEffect::LoadEarlier { thread, before } => self.load_earlier(thread, before).await,
            ServerEffect::Notify {
                thread,
                title,
                body,
            } => {
                platform::spawn_notify(thread, &title, body);
                Ok(())
            }
            ServerEffect::Attach {
                scope,
                target,
                source,
            } => {
                platform::spawn_attach(self.ui, scope, target, source);
                Ok(())
            }
            ServerEffect::SendInput { input, recovery } => self.send_input(input, recovery).await,
            ServerEffect::Fork {
                params,
                load_prompt,
            } => self.fork_thread(params, load_prompt).await,
            ServerEffect::ReadEntryChunk {
                thread,
                key,
                source,
            } => {
                self.read_entry_chunk(thread, key, source);
                Ok(())
            }
            ServerEffect::ReadFile {
                request,
                thread,
                project,
                path,
            } => {
                self.read_file_preview(request, thread, project, path);
                Ok(())
            }
            ServerEffect::ReadAttachment {
                thread,
                id,
                preview_request,
            } => {
                self.read_thread_attachment(thread, id, preview_request);
                Ok(())
            }
            ServerEffect::Reconnect | ServerEffect::Git { .. } | ServerEffect::Configure { .. } => {
                Ok(())
            }
        }
    }

    async fn select(&mut self, thread: ThreadId) -> viz_client::Result<()> {
        if let Err(error) = self.switch(Some(thread)).await {
            let previous = self.watched;
            let closed = matches!(error, Error::Closed);
            self.dispatch(ServerMsg::SelectFailed {
                failed: thread,
                previous,
                error: error.to_string(),
            });
            if closed {
                return Err(error);
            }
            return Ok(());
        }
        Ok(())
    }

    async fn on_notification(&self, notification: Notification) -> viz_client::Result<()> {
        match notification {
            Notification::ThreadsChanged(_) => self.refresh_list().await,
            Notification::TasksChanged(_) => self.refresh_tasks().await,
            Notification::HarnessesChanged(_) => self.refresh_harnesses().await,
            Notification::ThreadState { thread, state } => {
                if self.watched == Some(thread) {
                    self.dispatch(ServerMsg::ThreadState { thread, state });
                }
                Ok(())
            }
            Notification::ThreadLive { thread, delta } => {
                if self.watched == Some(thread) {
                    self.dispatch(ServerMsg::ThreadLive {
                        thread,
                        delta: Box::new(delta),
                    });
                }
                Ok(())
            }
            Notification::ThreadView {
                thread,
                entries,
                reset,
                before,
            } => {
                if self.watched == Some(thread) {
                    self.dispatch(ServerMsg::ThreadView {
                        thread,
                        entries,
                        reset,
                        before,
                    });
                }
                Ok(())
            }
            Notification::GitChanged { thread, status } => {
                if self.git_watched == Some(thread) {
                    self.dispatch(ServerMsg::Git(crate::update::GitMsg::Status(
                        thread, status,
                    )));
                }
                Ok(())
            }
            Notification::Reply { .. } => Ok(()),
        }
    }

    async fn switch(&mut self, thread: Option<ThreadId>) -> viz_client::Result<()> {
        if thread == self.watched {
            return Ok(());
        }
        if let Some(id) = thread {
            self.client.watch(id).await?;
        }
        let previous = self.watched;
        self.watched = thread;
        if let Some(previous) = previous
            && let Err(error) = self.client.unwatch(previous).await
        {
            if matches!(error, Error::Closed) {
                return Err(error);
            }
            toast(self.ui, "Couldn’t release thread watch", error.to_string());
        }
        Ok(())
    }
}
