//! Background server connection runtime: connects (retrying while offline),
//! watches the selected thread, and feeds typed [`crate::update::ServerMsg`]
//! events back into the UI.

use std::{cmp::Reverse, sync::Arc, time::Duration};

use rune::AppHandle;
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error,
    protocol::{
        CancelTask, ClientInfo, ClientKind, ConfigureThread, DeleteThread, Entry, EntryId,
        ListEnvironments, ListTasks, MoveThread, Notification, ThreadId,
    },
};

use super::{ServerEffect, platform, requests};
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

fn toast(ui: &AppHandle<Desktop>, title: &str, detail: String) {
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
                if !offline(ui, command, &mut None, &mut None) {
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
                    Some(command) => if !offline(ui, command, &mut selected, &mut environment) {
                        break;
                    },
                    None => return,
                },
            }
        }
    }
}

/// Handles a command while disconnected; returns `false` to reconnect now.
fn offline(
    ui: &AppHandle<Desktop>,
    command: ServerEffect,
    selected: &mut Option<ThreadId>,
    target: &mut Option<String>,
) -> bool {
    match command {
        ServerEffect::Select(thread) => *selected = Some(thread),
        ServerEffect::ClearSelection => *selected = None,
        ServerEffect::Environment {
            environment,
            thread,
        } => {
            *selected = thread;
            *target = Some(environment);
            return false;
        }
        ServerEffect::Create { draft, params } => {
            dispatch(
                ui,
                ServerMsg::LaunchFailed {
                    draft,
                    params: Box::new(params),
                    title: "Offline".into(),
                    detail: None,
                },
            );
        }
        ServerEffect::LoadDraft(draft) => {
            dispatch(
                ui,
                ServerMsg::DraftHarnessesLoaded {
                    draft,
                    result: None,
                },
            );
        }
        ServerEffect::SendInput(input) => {
            dispatch(
                ui,
                ServerMsg::SendFailed {
                    input: Box::new(input),
                    title: "Offline".into(),
                    detail: None,
                },
            );
        }
        ServerEffect::SearchFiles { .. } => {}
        ServerEffect::SearchThreads { request, .. } => {
            dispatch(
                ui,
                ServerMsg::ThreadSearchResult {
                    request,
                    result: Err("Offline".into()),
                },
            );
        }
        ServerEffect::LoadEarlier { thread, .. } => {
            dispatch(
                ui,
                ServerMsg::EarlierLoaded {
                    thread,
                    transcript: None,
                },
            );
        }
        ServerEffect::Update { op_id, .. }
        | ServerEffect::Project {
            op_id: Some(op_id), ..
        } => {
            dispatch(
                ui,
                ServerMsg::MutationFailed {
                    op_id,
                    title: "Offline".into(),
                    detail: "Server disconnected".into(),
                },
            );
        }
        ServerEffect::Attach {
            scope,
            target,
            source,
        } => platform::spawn_attach(ui, scope, target, source),
        ServerEffect::Notify { title, body } => platform::spawn_notify(&title, body),
        ServerEffect::Reconnect => return false,
        _ => toast(ui, "Offline", "Server disconnected".into()),
    }
    true
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
        entries: Vec::new(),
        before: None,
    };
    let started = async {
        let ((), harnesses, environments) = tokio::try_join!(
            session.refresh_list(),
            session.client.list_harnesses(),
            session.client.request(&ListEnvironments {}),
        )?;
        if let Some(thread) = *selected {
            match session.switch(Some(thread)).await {
                Err(Error::Rpc {
                    code: NOT_FOUND, ..
                }) => *selected = None,
                result => result?,
            }
        }
        let watched = *selected;
        session.dispatch(ServerMsg::Connected {
            version: hello.version,
            environment: hello.environment,
            environments: environments.environments,
            harnesses,
            watched,
        });
        Ok(())
    };
    if let Err(error) = started.await {
        return Ended::Lost(error);
    }
    signal(first);
    loop {
        let result = tokio::select! {
            notification = notifications.recv() => match notification {
                Some(notification) => session.on_notification(notification).await,
                None => Err(Error::Closed),
            },
            command = commands.recv() => match command {
                Some(ServerEffect::Environment { environment, thread }) => {
                    session.client.close();
                    return Ended::Environment { target: environment, thread };
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

/// Active WebSocket session state owned by the connection task.
pub(super) struct Session<'a> {
    pub(super) ui: &'a AppHandle<Desktop>,
    pub(super) client: Client,
    pub(super) environment: Arc<str>,
    pub(super) watched: Option<ThreadId>,
    pub(super) entries: Vec<Arc<Vec<Entry>>>,
    pub(super) before: Option<EntryId>,
}

impl Session<'_> {
    pub(super) fn dispatch(&self, msg: ServerMsg) {
        dispatch(self.ui, msg);
    }

    async fn on_command(
        &mut self,
        command: ServerEffect,
        selected: &mut Option<ThreadId>,
    ) -> viz_client::Result<()> {
        match command {
            ServerEffect::Select(thread) => {
                self.select(thread).await?;
                *selected = Some(thread);
                Ok(())
            }
            ServerEffect::ClearSelection => {
                self.switch(None).await?;
                *selected = None;
                Ok(())
            }
            ServerEffect::Create { draft, params } => self.launch(draft, params).await,
            ServerEffect::CancelTask(task) => {
                self.client.request(&CancelTask { task: &task }).await?;
                self.refresh_tasks().await
            }
            ServerEffect::Environment { .. } => {
                unreachable!("environment changes replace the session")
            }
            ServerEffect::LoadDraft(id) => {
                self.load_draft(id);
                Ok(())
            }
            ServerEffect::HarnessOptions { harness, model } => {
                self.harness_options(harness, model);
                Ok(())
            }
            ServerEffect::Update {
                op_id,
                thread,
                change,
            } => self.update_thread(op_id, thread, &change).await,
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
            ServerEffect::Configure {
                thread,
                option,
                value,
            } => {
                self.client
                    .configure_thread(&ConfigureThread {
                        thread,
                        option: &option,
                        value: &value,
                    })
                    .await
            }
            ServerEffect::Abort(id) => self.client.stop_thread(id).await,
            ServerEffect::Project { op_id, change } => self.project(op_id, change).await,
            ServerEffect::SearchFiles {
                thread,
                request,
                query,
            } => {
                self.search_files(thread, request, query);
                Ok(())
            }
            ServerEffect::SearchThreads { request, query } => {
                requests::search_threads(self.ui, &self.client, request, query);
                Ok(())
            }
            ServerEffect::LoadEarlier { thread, before } => self.load_earlier(thread, before).await,
            ServerEffect::Notify { title, body } => {
                platform::spawn_notify(&title, body);
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
            ServerEffect::SendInput(send_input) => self.send_input(send_input).await,
            ServerEffect::Reconnect => Ok(()),
        }
    }

    async fn select(&mut self, thread: ThreadId) -> viz_client::Result<()> {
        if let Err(error) = self.switch(Some(thread)).await {
            let previous = self.watched;
            let entries: Vec<Entry> = self
                .entries
                .iter()
                .flat_map(|page| page.iter().cloned())
                .collect();
            let before = self.before;
            let closed = matches!(error, Error::Closed);
            self.dispatch(ServerMsg::SelectFailed {
                failed: thread,
                previous,
                entries,
                before,
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
            Notification::ThreadsChanged(_) => self.refresh_list().await,
            Notification::TasksChanged(_) => self.refresh_tasks().await,
            Notification::HarnessesChanged(_) => {
                let harnesses = self.client.list_harnesses().await?;
                self.dispatch(ServerMsg::HarnessesSynced(harnesses));
                Ok(())
            }
            Notification::ThreadState { thread, state } => {
                self.dispatch(ServerMsg::ThreadState { thread, state });
                Ok(())
            }
            Notification::ThreadLive { thread, delta } => {
                self.dispatch(ServerMsg::ThreadLive {
                    thread,
                    delta: Box::new(delta),
                });
                Ok(())
            }
            Notification::ThreadView {
                thread,
                entries,
                reset,
                before,
            } => {
                if self.watched == Some(thread) {
                    if reset {
                        self.entries.clear();
                        self.before = before;
                    }
                    let entries = Arc::new(entries);
                    self.entries.push(Arc::clone(&entries));
                    self.dispatch(ServerMsg::ThreadView {
                        thread,
                        entries,
                        reset,
                        before,
                    });
                }
                Ok(())
            }
            Notification::Reply { .. }
            | Notification::GitChanged { .. }
            | Notification::TerminalUpdate { .. }
            | Notification::TerminalSnapshot { .. }
            | Notification::TerminalsChanged(_) => Ok(()),
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
        self.entries.clear();
        self.before = None;
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

    async fn refresh_tasks(&self) -> viz_client::Result<()> {
        let tasks = self.client.request(&ListTasks::default()).await?.tasks;
        self.dispatch(ServerMsg::TasksSynced {
            environment: self.environment.to_string(),
            tasks,
        });
        Ok(())
    }

    async fn refresh_list(&self) -> viz_client::Result<()> {
        let (mut threads, mut projects, ()) = tokio::try_join!(
            self.client.list_threads(),
            self.client.list_projects(),
            self.refresh_tasks(),
        )?;
        threads.sort_unstable_by_key(|thread| Reverse((thread.updated_at, thread.id)));
        projects.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
        self.dispatch(ServerMsg::WorkspaceSynced {
            environment: self.environment.to_string(),
            projects,
            threads,
        });
        Ok(())
    }
}
