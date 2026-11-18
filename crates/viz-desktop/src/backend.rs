//! The backend connection: a tokio runtime on its own thread that connects
//! (retrying while the backend is down), watches the selected conversation, and
//! pushes state into the UI.

use std::{collections::HashSet, time::Duration};

use rune::AppHandle;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error, delta,
    protocol::{ClientInfo, ClientKind, ConversationId, ConversationSummary, Notification, Submit},
};
use viz_config::Config;

use crate::{
    app::{Desktop, Status},
    thread::Thread,
};

/// How long to wait between connection attempts.
const RETRY: Duration = Duration::from_secs(3);

/// What the UI asks of the backend.
#[derive(Debug)]
pub enum Command {
    /// Watch this conversation instead.
    Select(ConversationId),
    /// Create a conversation and select it.
    Create,
    /// Submit an input to a conversation.
    Submit {
        /// The conversation.
        conversation: ConversationId,
        /// The input text.
        text: String,
    },
    /// While disconnected: try again now.
    Reconnect,
}

/// Starts the connection thread. `first` hears once the first connection
/// attempt has reached the UI (so a headless snapshot can show its outcome).
pub fn spawn(
    ui: AppHandle<Desktop>,
    commands: mpsc::UnboundedReceiver<Command>,
    first: std::sync::mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime.block_on(run(&ui, commands, first)),
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
    ui.update(move |s, _| s.status = status);
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
) {
    let mut first = Some(first);
    let endpoint = match Config::load()
        .map_err(Error::from)
        .and_then(|config| Endpoint::from_config(&config))
    {
        Ok(endpoint) => endpoint,
        Err(e) => {
            set_status(ui, Status::Error(e.to_string()));
            signal(&mut first);
            while let Some(command) = commands.recv().await {
                if !offline(ui, &command, &mut None) {
                    set_status(ui, Status::Error(e.to_string()));
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
                        toast(ui, "Lost the connection to the backend", e.to_string());
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
                    Some(command) => if !offline(ui, &command, &mut selected) {
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
    command: &Command,
    selected: &mut Option<ConversationId>,
) -> bool {
    match command {
        &Command::Select(conversation) => *selected = Some(conversation),
        Command::Create | Command::Submit { .. } => toast(
            ui,
            "Not connected",
            "Start the backend or check [client].url in the config.".into(),
        ),
        Command::Reconnect => return false,
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
    *selected = Some(watched);
    let mut session = Session {
        ui,
        client,
        watched,
        view: None,
        listed: HashSet::new(),
    };
    let (version, root) = (hello.version, hello.root);
    ui.update(move |s, _| {
        s.status = Status::Connected { version };
        s.root = Some(root);
        s.show(watched);
    });
    let started = async {
        session.refresh_list().await?;
        session.watch().await
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
            Err(e) => toast(ui, "Request failed", e.to_string()),
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
    /// Conversations the sidebar lists, to notice new subagents.
    listed: HashSet<ConversationId>,
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
            Command::Create => {
                #[derive(Deserialize)]
                struct Created {
                    conversation: ConversationId,
                }
                let Created { conversation } =
                    self.client.call("conversation.create", json!({})).await?;
                self.refresh_list().await?;
                *selected = Some(conversation);
                self.ui.update(move |s, _| {
                    s.show(conversation);
                    s.draft.focused = true;
                });
                self.switch(conversation).await
            }
            Command::Submit { conversation, text } => {
                let submit = Submit {
                    conversation: Some(conversation),
                    text,
                    ..Submit::default()
                };
                self.client.submit(&submit).await?;
                // A chat's first message titles it.
                self.refresh_list().await
            }
            // Already connected.
            Command::Reconnect => Ok(()),
        }
    }

    async fn on_notification(&mut self, notification: Notification) -> viz_client::Result<()> {
        let Notification::ConversationView {
            conversation,
            value,
            ops,
        } = notification
        else {
            return Ok(());
        };
        if conversation != self.watched {
            return Ok(());
        }
        match (value, ops, &mut self.view) {
            (Some(value), _, view) => *view = Some(value),
            (None, Some(Value::Array(ops)), Some(view)) => {
                if let Err(e) = delta::apply(view, &ops) {
                    // The replica is unusable: start over from a fresh full
                    // value.
                    toast(self.ui, "View out of sync; reloading", e.to_string());
                    self.view = None;
                    self.unwatch().await?;
                    return self.watch().await;
                }
            }
            // Ops before the first value, or not a batch: nothing to apply them
            // to.
            _ => return Ok(()),
        }
        let Some(view) = &self.view else {
            return Ok(());
        };
        let thread = Thread::from_view(view);
        let unlisted: Vec<_> = thread
            .children()
            .filter(|child| !self.listed.contains(child))
            .collect();
        self.ui
            .update(move |s, _| s.set_thread(conversation, thread));
        if !unlisted.is_empty() {
            self.refresh_list().await?;
            // Asked once: a child the list still lacks doesn't ask again on
            // every revision.
            self.listed.extend(unlisted);
        }
        Ok(())
    }

    async fn switch(&mut self, conversation: ConversationId) -> viz_client::Result<()> {
        if conversation == self.watched {
            return Ok(());
        }
        self.unwatch().await?;
        self.watched = conversation;
        self.view = None;
        self.watch().await
    }

    async fn watch(&self) -> viz_client::Result<()> {
        self.client
            .call::<Value>(
                "conversation.watch",
                json!({ "conversation": self.watched }),
            )
            .await
            .map(drop)
    }

    async fn unwatch(&self) -> viz_client::Result<()> {
        self.client
            .call::<Value>(
                "conversation.unwatch",
                json!({ "conversation": self.watched }),
            )
            .await
            .map(drop)
    }

    /// Lists every conversation, titled by the backend, into the sidebar.
    async fn refresh_list(&mut self) -> viz_client::Result<()> {
        #[derive(Deserialize)]
        struct Listing {
            conversations: Vec<ConversationSummary>,
        }
        let Listing { conversations } = self.client.call("conversations.list", json!({})).await?;
        self.listed = conversations.iter().map(|summary| summary.id).collect();
        self.ui.update(move |s, _| s.conversations = conversations);
        Ok(())
    }
}
