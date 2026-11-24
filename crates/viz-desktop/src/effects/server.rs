//! Background server connection runtime: connects (retrying while offline),
//! watches the selected thread, and feeds typed [`crate::update::ServerMsg`]
//! events back into the UI.

use std::{sync::Arc, time::Duration};

use rune::AppHandle;
use tokio::{sync::mpsc, task::JoinSet};
use viz_client::{
    Client, Connection, ContentClient, Endpoint, Error,
    protocol::{
        CancelTask, ClientInfo, ClientKind, ConfigureThread, DeleteThread, ListEnvironments,
        ListTasks, MoveThread, Notification, ThreadId,
    },
};

use super::{ServerEffect, git, offline, platform};
use crate::{
    app::Desktop,
    model::{SessionId, Status},
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
                dispatch_global(
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

pub(super) fn dispatch_global(ui: &AppHandle<Desktop>, msg: ServerMsg) {
    ui.update(move |s, tx| s.dispatch(Msg::Server(msg), tx));
}

fn dispatch_session(ui: &AppHandle<Desktop>, session: SessionId, msg: ServerMsg) {
    ui.update(move |s, tx| {
        s.dispatch(
            Msg::Session {
                session,
                msg: Box::new(msg),
            },
            tx,
        );
    });
}

pub(super) fn toast(ui: &AppHandle<Desktop>, title: &str, detail: String) {
    dispatch_global(
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
            dispatch_global(ui, ServerMsg::StatusChanged(Status::Error(detail.clone())));
            signal(&mut first);
            while let Some(command) = commands.recv().await {
                if !offline::handle(ui, command, &mut None, &mut None) {
                    dispatch_global(ui, ServerMsg::StatusChanged(Status::Error(detail.clone())));
                }
            }
            return;
        }
    };
    dispatch_global(ui, ServerMsg::Endpoint(endpoint.to_string()));
    let mut selected = None;
    let mut environment = None;
    let mut next_session = 0_u64;
    loop {
        let info = ClientInfo {
            kind: ClientKind::App,
            name: "desktop".into(),
        };
        let lost = match Client::connect(&endpoint, info.clone(), environment.clone()).await {
            Ok(connection) => {
                let target_env = Some(connection.hello.environment.clone());
                match ContentClient::connect(&endpoint, info, target_env).await {
                    Ok(content) => {
                        next_session = next_session.wrapping_add(1).max(1);
                        let session_id = SessionId(next_session);
                        match session(
                            ui,
                            session_id,
                            connection,
                            content,
                            &mut commands,
                            &mut selected,
                            &mut first,
                        )
                        .await
                        {
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
                    Err(error) => {
                        connection.client.close();
                        error.to_string()
                    }
                }
            }
            Err(error) => error.to_string(),
        };
        tracing::debug!(%endpoint, "server connection unavailable; retrying");
        dispatch_global(ui, ServerMsg::StatusChanged(Status::Error(lost)));
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
    id: SessionId,
    connection: Connection,
    content: ContentClient,
    commands: &mut mpsc::UnboundedReceiver<ServerEffect>,
    selected: &mut Option<ThreadId>,
    first: &mut Option<std::sync::mpsc::Sender<()>>,
) -> Ended {
    let Connection {
        client,
        notifications,
        hello,
    } = connection;
    let mut session = Session {
        id,
        ui,
        client,
        content,
        environment: Arc::from(hello.environment.as_str()),
        workspace_rev: 0,
        watched: None,
        git_watched: None,
        jobs: JoinSet::new(),
    };
    if let Err(error) = session.start(hello, selected).await {
        session.close();
        return Ended::Lost(error);
    }
    signal(first);
    let ended = session.drive(notifications, commands, selected).await;
    session.close();
    ended
}

pub(super) enum BackgroundReply {
    Msg(ServerMsg),
    OptionalMsg(Option<ServerMsg>),
    Configured(viz_client::Result<()>),
    Git { reply: ServerMsg, refresh: bool },
}

/// Active WebSocket session state owned by the connection task.
pub(super) struct Session<'a> {
    pub(super) id: SessionId,
    pub(super) ui: &'a AppHandle<Desktop>,
    pub(super) client: Client,
    pub(super) content: ContentClient,
    pub(super) environment: Arc<str>,
    pub(super) workspace_rev: u64,
    pub(super) watched: Option<ThreadId>,
    pub(super) git_watched: Option<ThreadId>,
    pub(super) jobs: JoinSet<BackgroundReply>,
}

impl Session<'_> {
    fn close(&self) {
        self.client.close();
        self.content.close();
    }

    async fn drive(
        &mut self,
        mut notifications: mpsc::Receiver<Notification>,
        commands: &mut mpsc::UnboundedReceiver<ServerEffect>,
        selected: &mut Option<ThreadId>,
    ) -> Ended {
        let mut configs: std::collections::VecDeque<(
            ThreadId,
            String,
            viz_client::protocol::OptionValue,
        )> = std::collections::VecDeque::new();
        let mut configuring = false;
        loop {
            if !configuring && let Some((thread, option, value)) = configs.pop_front() {
                configuring = true;
                let client = self.client.clone();
                self.jobs.spawn(async move {
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
                    Some(notification) => self.on_notification(notification).await,
                    None => Err(Error::Closed),
                },
                done = self.jobs.join_next(), if !self.jobs.is_empty() => match done {
                    Some(Ok(
                        BackgroundReply::Msg(msg) | BackgroundReply::OptionalMsg(Some(msg)),
                    )) => {
                        self.dispatch(msg);
                        Ok(())
                    }
                    Some(Ok(BackgroundReply::OptionalMsg(None))) | None => Ok(()),
                    Some(Ok(BackgroundReply::Configured(result))) => {
                        configuring = false;
                        result
                    }
                    Some(Ok(BackgroundReply::Git { reply, refresh })) => {
                        self.dispatch(reply);
                        if refresh { self.refresh_list().await } else { Ok(()) }
                    }
                    Some(Err(error)) => {
                        tracing::error!(%error, "desktop request task ended unexpectedly");
                        return Ended::Lost(Error::Closed);
                    }
                },
                command = commands.recv() => match command {
                    Some(ServerEffect::Environment { environment, thread }) => {
                        return Ended::Environment { target: environment, thread };
                    }
                    Some(ServerEffect::Git { request: git::Request::Watch(next), .. }) => self.watch_git(next).await,
                    Some(ServerEffect::Git { thread, serial, request }) => {
                        if self.watched == Some(thread) {
                            let client = self.client.clone();
                            let refresh = matches!(&request, git::Request::Init(_) | git::Request::CreatePull { .. });
                            self.jobs.spawn(async move {
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
                    Some(command) => self.on_command(command, selected).await,
                    None => return Ended::Quit,
                },
            };
            match result {
                Ok(()) => {}
                Err(Error::Closed) => return Ended::Lost(Error::Closed),
                Err(error) => toast(self.ui, "Request failed", error.to_string()),
            }
        }
    }

    pub(super) fn dispatch(&self, msg: ServerMsg) {
        dispatch_session(self.ui, self.id, msg);
    }

    pub(super) fn spawn_msg<F>(&mut self, fut: F)
    where
        F: Future<Output = ServerMsg> + Send + 'static,
    {
        self.jobs
            .spawn(async move { BackgroundReply::Msg(fut.await) });
    }

    pub(super) fn spawn_opt_msg<F>(&mut self, fut: F)
    where
        F: Future<Output = Option<ServerMsg>> + Send + 'static,
    {
        self.jobs
            .spawn(async move { BackgroundReply::OptionalMsg(fut.await) });
    }

    async fn start(
        &mut self,
        hello: viz_client::protocol::Hello,
        selected: &mut Option<ThreadId>,
    ) -> viz_client::Result<()> {
        let list_tasks = ListTasks::default();
        let list_envs = ListEnvironments {};
        let (thread_list, project_list, task_list, harnesses, environments) = tokio::try_join!(
            self.client.list_threads_snapshot(),
            self.client.list_projects_snapshot(),
            self.client.request(&list_tasks),
            self.client.list_harnesses(),
            self.client.request(&list_envs),
        )?;
        let rev = thread_list.rev.min(project_list.rev).min(task_list.rev);
        self.workspace_rev = rev;
        let mut projects = project_list.projects;
        projects.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        if let Some(thread) = *selected {
            match self.switch(Some(thread)).await {
                Err(Error::Rpc {
                    code: NOT_FOUND, ..
                }) => *selected = None,
                result => result?,
            }
        }
        dispatch_global(
            self.ui,
            ServerMsg::Connected {
                session: self.id,
                version: hello.version,
                environment: hello.environment,
                local_host: viz_client::protocol::HostId::local().as_ref() == Some(&hello.host),
                rev,
                environments: environments.environments,
                harnesses,
                projects,
                threads: thread_list.threads,
                tasks: task_list.tasks,
                watched: *selected,
            },
        );
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
                attachments,
                recovery,
            } => self.launch(draft, *params, attachments, recovery).await,
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
                self.search_threads(request, query);
                Ok(())
            }
            ServerEffect::LoadHistory { thread, anchor } => self.load_history(thread, anchor).await,
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
            ServerEffect::SendInput {
                input,
                attachments,
                recovery,
            } => self.send_input(input, attachments, recovery).await,
            ServerEffect::Fork {
                params,
                load_prompt,
            } => self.fork_thread(params, load_prompt).await,
            ServerEffect::ReadEntry { thread, key, entry } => {
                self.read_entry(thread, key, entry);
                Ok(())
            }
            ServerEffect::ReadFile {
                request,
                root,
                path,
                window,
            } => {
                self.read_file_preview(request, root, path, window);
                Ok(())
            }
            ServerEffect::OpenLocalFile { directory, path } => {
                self.open_local_file(directory, path);
                Ok(())
            }
            ServerEffect::DownloadFile { root, path } => {
                self.download_file(root, path);
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

    async fn on_notification(&mut self, notification: Notification) -> viz_client::Result<()> {
        match notification {
            Notification::WorkspaceDelta {
                base_rev,
                rev,
                changes,
            } => {
                if rev <= self.workspace_rev {
                    return Ok(());
                }
                let needs_resync = base_rev > self.workspace_rev
                    || changes.iter().any(|change| {
                        matches!(change, viz_client::protocol::WorkspaceChange::Resync)
                    });
                if needs_resync {
                    self.refresh_list().await
                } else {
                    self.workspace_rev = rev;
                    self.dispatch(ServerMsg::WorkspaceDelta {
                        environment: self.environment.to_string(),
                        rev,
                        changes,
                    });
                    Ok(())
                }
            }
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
