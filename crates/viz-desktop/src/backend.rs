//! The backend connection: a tokio runtime on its own thread that connects
//! (retrying while the backend is down), watches the selected conversation, and
//! pushes state into the UI.

mod requests;

use std::{cmp::Reverse, time::Duration};

use rune::AppHandle;
use serde_json::Value;
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error, delta,
    protocol::{
        ClientInfo, ClientKind, ConfigureConversation, ConversationId, CreateConversation,
        CreateProject, EntryId, HarnessOptions, Notification, OptionValue, RemoveProject,
        RemoveQueuedInput, Revision, SearchFiles, SubmissionId, Submit, UpdateConversation,
        UpdateProject, ViewRevision,
    },
};

use self::requests::History;
use crate::{
    app::{Desktop, Status},
    attachments::{Source, Target as AttachmentTarget},
    thread::Thread,
};

/// How long to wait between connection attempts.
const RETRY: Duration = Duration::from_secs(3);
const NOT_FOUND: i64 = -32001;

/// What the UI asks of the backend.
#[derive(Debug)]
pub enum Command {
    /// Watch this conversation instead.
    Select(ConversationId),
    /// Bind and send the first input of the fresh composer.
    Create {
        draft: u64,
        params: CreateConversation,
        submit: Submit,
    },
    /// A runner's options with `model` selected, for a draft that picked a
    /// model whose thinking levels the catalog doesn't know yet.
    HarnessOptions {
        harness: String,
        model: String,
    },
    /// A fresh draft's runner choices.
    LoadDraft(u64),
    Update(ConversationId, ChatChange),
    Delete(ConversationId),
    Configure {
        conversation: ConversationId,
        option: String,
        value: OptionValue,
    },
    Abort(ConversationId),
    Project(ProjectChange),
    SearchFiles {
        conversation: ConversationId,
        request: u64,
        query: String,
    },
    SearchConversations {
        request: u64,
        query: String,
    },
    LoadEarlier {
        conversation: ConversationId,
        before: EntryId,
    },
    RemoveQueued {
        conversation: ConversationId,
        submission: SubmissionId,
    },
    Attach {
        target: AttachmentTarget,
        source: Source,
    },
    /// Submit an input, including any locally prepared attachments.
    Submit(Submit),
    /// Show a desktop notification.
    Notify {
        title: String,
        body: &'static str,
    },
    /// While disconnected: try again now.
    Reconnect,
}

#[derive(Debug)]
pub enum ChatChange {
    Title(String),
    Archive(bool),
    Pin(bool),
    Unread(bool),
    Project(Option<String>),
}

#[derive(Debug)]
pub enum ProjectChange {
    Add(String),
    Rename { id: String, name: String },
    Remove(String),
}

/// Starts the connection thread. `first` hears once the first connection
/// attempt has reached the UI (so a headless snapshot can show its outcome).
pub fn spawn(
    ui: AppHandle<Desktop>,
    commands: mpsc::UnboundedReceiver<Command>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run(&ui, commands, first, endpoint)),
            Err(e) => {
                set_status(
                    &ui,
                    Status::Error(format!("starting the connection thread: {e}")),
                );
                _ = first.send(());
            }
        }
    });
}

fn set_status(ui: &AppHandle<Desktop>, status: Status) {
    ui.update(move |s, tx| {
        s.status = status;
        s.update_title(tx);
    });
}

fn toast(ui: &AppHandle<Desktop>, title: &'static str, detail: String) {
    ui.update(move |s, tx| s.toast(tx, title, Some(detail)));
}

/// Tells `main` the first connection attempt's outcome is in the UI's queue.
fn signal(first: &mut Option<std::sync::mpsc::Sender<()>>) {
    if let Some(first) = first.take() {
        _ = first.send(());
    }
}

async fn run(
    ui: &AppHandle<Desktop>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    first: std::sync::mpsc::Sender<()>,
    endpoint: Result<Endpoint, String>,
) {
    let mut first = Some(first);
    let endpoint = match endpoint {
        Ok(endpoint) => endpoint,
        Err(detail) => {
            set_status(ui, Status::Error(detail.clone()));
            signal(&mut first);
            while let Some(command) = commands.recv().await {
                if !offline(ui, command, &mut None) {
                    set_status(ui, Status::Error(detail.clone()));
                }
            }
            return;
        }
    };
    let shown = endpoint.to_string();
    ui.update(move |s, _| s.endpoint = Some(shown));
    // Survives reconnects, so the window keeps its conversation.
    let mut selected = None;
    loop {
        let info = ClientInfo {
            kind: ClientKind::App,
            name: "desktop".into(),
        };
        let lost = match Client::connect(&endpoint, info).await {
            Ok(connection) => {
                match session(ui, connection, &mut commands, &mut selected, &mut first).await {
                    Ended::Quit => return,
                    Ended::Lost(e) => {
                        toast(ui, "lost the connection to the backend", e.to_string());
                        e.to_string()
                    }
                }
            }
            Err(e) => e.to_string(),
        };
        set_status(ui, Status::Error(lost));
        signal(&mut first);
        let retry = tokio::time::sleep(RETRY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = &mut retry => break,
                command = commands.recv() => match command {
                    Some(command) => if !offline(ui, command, &mut selected) {
                        break;
                    },
                    None => return,
                },
            }
        }
    }
}

/// Handles a command while disconnected; false to try connecting now.
fn offline(
    ui: &AppHandle<Desktop>,
    command: Command,
    selected: &mut Option<ConversationId>,
) -> bool {
    match command {
        Command::Select(conversation) => *selected = Some(conversation),
        Command::Create { draft, submit, .. } => {
            ui.update(move |s, tx| {
                s.restore_first_message(draft, submit);
                s.toast(tx, "offline", None);
            });
        }
        Command::LoadDraft(id) => ui.update(move |s, _| s.draft_loaded(id)),
        Command::Submit(submit) => {
            ui.update(move |s, tx| {
                s.restore_message(submit);
                s.toast(tx, "offline", None);
            });
        }
        Command::SearchFiles { .. } => {}
        Command::SearchConversations { request, .. } => {
            ui.update(move |s, _| s.palette.set_hits(request, Err("offline".into())));
        }
        Command::LoadEarlier { conversation, .. } => {
            ui.update(move |s, tx| s.loaded_earlier(conversation, None, tx));
        }
        Command::Attach { target, source } => requests::attach(ui, target, source),
        Command::Notify { title, body } => requests::notify(&title, body),
        Command::Reconnect => return false,
        _ => toast(ui, "offline", "backend disconnected".into()),
    }
    true
}

/// Why a session ended.
enum Ended {
    /// The UI is gone.
    Quit,
    /// The connection failed.
    Lost(Error),
}

/// One connection's life: list, watch, and serve commands until it closes.
async fn session(
    ui: &AppHandle<Desktop>,
    connection: Connection,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    selected: &mut Option<ConversationId>,
    first: &mut Option<std::sync::mpsc::Sender<()>>,
) -> Ended {
    let Connection {
        client,
        mut notifications,
        hello,
    } = connection;
    let watched = selected.unwrap_or(hello.root);
    let mut session = Session {
        ui,
        client,
        watched,
        view: None,
        history: None,
    };
    let (version, root) = (hello.version, hello.root);
    let started = async {
        let watch = async {
            match session.client.watch(watched).await {
                Err(Error::Rpc {
                    code: NOT_FOUND, ..
                }) if watched != root => {
                    session.client.watch(root).await?;
                    Ok(root)
                }
                result => result.map(|()| watched),
            }
        };
        let ((), harnesses, watched) = tokio::try_join!(
            session.refresh_list(),
            session.client.list_harnesses(),
            watch
        )?;
        session.watched = watched;
        *selected = Some(watched);
        ui.update(move |s, tx| {
            s.status = Status::Connected { version };
            s.root = Some(root);
            s.harnesses = harnesses;
            if s.new_chat.is_none() {
                s.show(watched);
                tx.focus("draft", false);
            }
            s.update_title(tx);
        });
        Ok(())
    };
    if let Err(e) = started.await {
        return Ended::Lost(e);
    }
    signal(first);
    loop {
        let result = tokio::select! {
            notification = notifications.recv() => match notification {
                Some(notification) => session.on_notification(notification).await,
                None => Err(Error::Closed),
            },
            command = commands.recv() => match command {
                Some(command) => session.on_command(command, selected).await,
                None => return Ended::Quit,
            },
        };
        match result {
            Ok(()) => {}
            Err(Error::Closed) => return Ended::Lost(Error::Closed),
            Err(e) => toast(ui, "request failed", e.to_string()),
        }
    }
}

struct Session<'a> {
    ui: &'a AppHandle<Desktop>,
    client: Client,
    /// The conversation whose view this session holds.
    watched: ConversationId,
    /// Its latest view, once the first value arrives.
    view: Option<Value>,
    history: Option<History>,
}

impl Session<'_> {
    async fn on_command(
        &mut self,
        command: Command,
        selected: &mut Option<ConversationId>,
    ) -> viz_client::Result<()> {
        match command {
            Command::Select(conversation) => {
                *selected = Some(conversation);
                self.switch(conversation).await
            }
            Command::Create {
                draft,
                params,
                mut submit,
            } => {
                let conversation = match self.client.create_conversation(&params).await {
                    Ok(id) => id,
                    Err(error) => {
                        self.ui
                            .update(move |s, _| s.restore_first_message(draft, submit));
                        return Err(error);
                    }
                };
                self.ui
                    .update(move |s, tx| s.created(draft, conversation, tx));
                submit.conversation = Some(conversation);
                self.submit(submit).await
            }
            Command::LoadDraft(id) => {
                self.load_draft(id);
                Ok(())
            }
            Command::HarnessOptions { harness, model } => {
                self.harness_options(harness, model);
                Ok(())
            }
            Command::Update(conversation, change) => self.update_chat(conversation, &change).await,
            Command::Delete(conversation) => self.client.delete_conversation(conversation).await,
            Command::Configure {
                conversation,
                option,
                value,
            } => {
                self.client
                    .configure_conversation(&ConfigureConversation {
                        conversation,
                        option: &option,
                        value: &value,
                    })
                    .await
            }
            Command::Abort(id) => self.client.abort_conversation(id).await,
            Command::Project(change) => self.project(change).await,
            Command::SearchFiles {
                conversation,
                request,
                query,
            } => {
                self.search(conversation, request, query);
                Ok(())
            }
            Command::SearchConversations { request, query } => {
                requests::search(self.ui, &self.client, request, query);
                Ok(())
            }
            Command::LoadEarlier {
                conversation,
                before,
            } => self.load_earlier(conversation, before).await,
            Command::RemoveQueued {
                conversation,
                submission,
            } => {
                self.client
                    .remove_queued_input(&RemoveQueuedInput {
                        conversation,
                        submission,
                    })
                    .await
            }
            Command::Notify { title, body } => {
                requests::notify(&title, body);
                Ok(())
            }
            Command::Attach { target, source } => {
                requests::attach(self.ui, target, source);
                Ok(())
            }
            Command::Submit(submit) => self.submit(submit).await,
            // Already connected.
            Command::Reconnect => Ok(()),
        }
    }

    async fn submit(&self, submit: Submit) -> viz_client::Result<()> {
        let result = self.client.submit(&submit).await;
        if result.is_err() {
            self.ui.update(move |s, _| s.restore_message(submit));
        }
        result.map(drop)
    }

    fn load_draft(&self, id: u64) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let harnesses = client.list_harnesses().await;
            ui.update(move |s, tx| {
                if s.new_chat.as_ref().is_none_or(|chat| chat.id != id) {
                    return;
                }
                match harnesses {
                    Ok(harnesses) => s.harnesses = harnesses,
                    Err(error) => s.toast(tx, "couldn't load harnesses", Some(error.to_string())),
                }
                s.draft_loaded(id);
            });
        });
    }

    async fn update_chat(
        &self,
        conversation: ConversationId,
        change: &ChatChange,
    ) -> viz_client::Result<()> {
        let mut params = UpdateConversation {
            conversation,
            title: None,
            archived: None,
            pinned: None,
            unread: None,
            project: None,
        };
        match change {
            ChatChange::Title(title) => params.title = Some(title),
            ChatChange::Archive(archived) => params.archived = Some(*archived),
            ChatChange::Pin(pinned) => params.pinned = Some(*pinned),
            ChatChange::Unread(unread) => params.unread = Some(*unread),
            ChatChange::Project(project) => params.project = Some(project.as_deref()),
        }
        self.client.update_conversation(&params).await
    }

    async fn on_notification(&mut self, notification: Notification) -> viz_client::Result<()> {
        match notification {
            Notification::ConversationsChanged(_) => self.refresh_list().await,
            Notification::ConversationState {
                conversation,
                state,
            } => {
                self.ui
                    .update(move |s, tx| s.set_conversation(conversation, state, tx));
                Ok(())
            }
            Notification::ConversationView(view) => self.on_view(view).await,
            Notification::Reply { .. } => Ok(()),
        }
    }

    async fn on_view(
        &mut self,
        ViewRevision {
            conversation,
            revision,
        }: ViewRevision,
    ) -> viz_client::Result<()> {
        if conversation != self.watched {
            return Ok(());
        }
        let view = match (revision, &mut self.view) {
            (Revision::Full(value), view) => view.insert(value),
            (Revision::Delta(ops), Some(view)) => {
                let result = if let Some(history) = &mut self.history {
                    history.apply(view, &ops)
                } else {
                    delta::apply(view, &ops)
                };
                if let Err(e) = result {
                    // The replica is unusable: start over from a fresh full
                    // value.
                    toast(self.ui, "view out of sync; reloading", e.to_string());
                    self.view = None;
                    self.history = None;
                    self.client.unwatch(conversation).await?;
                    return self.client.watch(conversation).await;
                }
                view
            }
            // Ignore deltas until a full view arrives.
            (Revision::Delta(_), None) => return Ok(()),
        };
        let thread = self
            .history
            .as_ref()
            .map_or_else(|| Thread::from_view(view), |history| history.thread(view));
        self.ui
            .update(move |s, tx| s.set_thread(conversation, thread, tx));
        Ok(())
    }

    async fn switch(&mut self, conversation: ConversationId) -> viz_client::Result<()> {
        if conversation == self.watched {
            return Ok(());
        }
        self.client.unwatch(self.watched).await?;
        self.watched = conversation;
        self.view = None;
        self.history = None;
        self.client.watch(conversation).await
    }

    async fn refresh_list(&self) -> viz_client::Result<()> {
        let (mut conversations, mut projects) = tokio::try_join!(
            self.client.list_conversations(),
            self.client.list_projects()
        )?;
        conversations.sort_unstable_by_key(|chat| Reverse((chat.updated_at, chat.id)));
        projects.sort_unstable_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        self.ui.update(move |s, tx| {
            s.set_conversations(conversations, tx);
            s.projects = projects;
        });
        Ok(())
    }

    async fn project(&self, change: ProjectChange) -> viz_client::Result<()> {
        match &change {
            ProjectChange::Add(path) => {
                let project = self
                    .client
                    .create_project(&CreateProject { path, name: None })
                    .await?;
                self.ui.update(move |s, _| s.project_added(project));
                Ok(())
            }
            ProjectChange::Rename { id, name } => self
                .client
                .update_project(&UpdateProject {
                    project: id,
                    name: Some(name),
                })
                .await
                .map(drop),
            ProjectChange::Remove(id) => {
                self.client
                    .remove_project(&RemoveProject { project: id })
                    .await
            }
        }
    }

    fn search(&self, conversation: ConversationId, request: u64, query: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .search_files(&SearchFiles {
                    conversation,
                    query: &query,
                    limit: Some(24),
                })
                .await;
            ui.update(move |s, _| {
                if s.selected == Some(conversation) {
                    s.completion.file_result(request, result);
                }
            });
        });
    }

    fn harness_options(&self, harness: String, model: String) {
        let ui = self.ui.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .harness_options(&HarnessOptions {
                    harness: harness.clone(),
                    model,
                })
                .await;
            ui.update(move |s, tx| match result {
                Ok(result) => s.set_harness_options(&harness, result.options),
                Err(error) => s.toast(tx, "couldn't load model options", Some(error.to_string())),
            });
        });
    }
}
