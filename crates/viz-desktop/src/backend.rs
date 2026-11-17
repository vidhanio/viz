//! The backend connection: a tokio runtime on its own thread that connects
//! (retrying while the backend is down), watches the selected conversation, and
//! pushes state into the UI.

use std::time::Duration;

use rune::AppHandle;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use viz_client::{
    Client, Connection, Endpoint, Error, delta,
    protocol::{ClientInfo, ClientKind, ConversationId, Notification, Submit},
};
use viz_config::Config;

use crate::{Desktop, Status, transcript};

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

fn notify(ui: &AppHandle<Desktop>, notice: String) {
    ui.update(move |s, _| s.notice = Some(notice));
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
                offline(ui, &command, &mut None);
            }
            return;
        }
    };
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
                    Ended::Lost(e) => e.to_string(),
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
                    Some(command) => offline(ui, &command, &mut selected),
                    None => return,
                },
            }
        }
    }
}

/// Handles a command while disconnected.
fn offline(ui: &AppHandle<Desktop>, command: &Command, selected: &mut Option<ConversationId>) {
    match command {
        &Command::Select(conversation) => *selected = Some(conversation),
        Command::Create | Command::Submit { .. } => {
            notify(ui, "Not connected to the backend".into());
        }
    }
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
        root: hello.root,
        watched,
        view: None,
    };
    let (version, root) = (hello.version, hello.root);
    ui.update(move |s, _| {
        s.status = Status::Connected { version };
        s.root = Some(root);
        s.show(watched);
        s.notice = None;
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
            Err(e) => notify(ui, e.to_string()),
        }
    }
}

struct Session<'a> {
    ui: &'a AppHandle<Desktop>,
    client: Client,
    root: ConversationId,
    /// The conversation whose view this session holds.
    watched: ConversationId,
    /// Its latest view, once the first value arrives.
    view: Option<Value>,
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
                self.ui.update(move |s, _| s.show(conversation));
                self.switch(conversation).await
            }
            Command::Submit { conversation, text } => {
                let submit = Submit {
                    conversation: Some(conversation),
                    text,
                    ..Submit::default()
                };
                self.client.submit(&submit).await.map(drop)
            }
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
                    notify(self.ui, format!("view out of sync ({e}); reloading"));
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
        let lines = transcript::lines(view);
        self.ui.update(move |s, _| {
            if s.selected == Some(conversation) {
                s.transcript = lines;
            }
        });
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

    /// Lists conversations other than the root into the sidebar.
    async fn refresh_list(&self) -> viz_client::Result<()> {
        #[derive(Deserialize)]
        struct Listed {
            conversations: Vec<Record>,
        }
        #[derive(Deserialize)]
        struct Record {
            id: ConversationId,
        }
        let Listed { conversations } = self.client.call("conversations.list", json!({})).await?;
        let root = self.root;
        let mut ids: Vec<_> = conversations
            .into_iter()
            .map(|record| record.id)
            .filter(|&id| id != root)
            .collect();
        ids.sort_unstable();
        self.ui.update(move |s, _| s.conversations = ids);
        Ok(())
    }
}
