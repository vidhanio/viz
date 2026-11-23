//! Background server connection runtime: connects (retrying while offline),
//! watches the selected thread, and feeds typed [`crate::update::ServerMsg`]
//! events back into the UI.

use std::{sync::Arc, time::Duration};

use rune::AppHandle;
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error,
    protocol::{
        CancelTask, ClientInfo, ClientKind, ConfigureThread, DeleteThread, Entry, EntryId,
        ListEnvironments, MoveThread, Notification, ThreadId,
    },
};

use super::{ServerEffect, offline, platform, requests};
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
        session
            .terminal(crate::update::terminal::Command::List)
            .await?;
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
                Some(ServerEffect::Git { thread, serial, request }) => session.git(thread, serial, request).await,
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
    pub(super) git_watched: Option<ThreadId>,
    pub(super) entries: Vec<Arc<Vec<Entry>>>,
    pub(super) before: Option<EntryId>,
}

impl Session<'_> {
    pub(super) fn dispatch(&self, msg: ServerMsg) {
        dispatch(self.ui, msg);
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
            ServerEffect::Terminal(command) => self.terminal(command).await,
            ServerEffect::Answer(answer) => self.answer(answer).await,
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
            ServerEffect::Reconnect | ServerEffect::Git { .. } => Ok(()),
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
            Notification::HarnessesChanged(_) => self.refresh_harnesses().await,
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
            Notification::GitChanged { thread, status } => {
                if self.git_watched == Some(thread) {
                    self.dispatch(ServerMsg::Git(crate::update::GitMsg::Status(
                        thread, status,
                    )));
                }
                Ok(())
            }
            Notification::TerminalUpdate { id, update } => {
                self.terminal_event(super::terminal::Event::Update(id, update));
                Ok(())
            }
            Notification::TerminalSnapshot { id, snapshot } => {
                self.terminal_event(super::terminal::Event::Snapshot(id, snapshot));
                Ok(())
            }
            Notification::TerminalsChanged(_) => {
                self.terminal(crate::update::terminal::Command::List).await
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
}
