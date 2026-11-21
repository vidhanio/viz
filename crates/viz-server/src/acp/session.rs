//! A thread's ACP session: one harness process, its options, its turns, and
//! native session import.

use std::{
    fmt::Write as _,
    future::Future,
    mem,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use agent_client_protocol::{
    schema::MaybeUndefined,
    v1::schema::{
        AvailableCommand, AvailableCommandInput, SessionConfigOption, SessionNotification,
        SessionUpdate, StopReason,
    },
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, watch},
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    bridge,
    mcp::McpEndpoint,
    options::{
        Desired, Selection, defaults, model_option, normalize, reconcile, thinking_option,
        valid_value,
    },
    process::{Agent, UpdateSink},
    tool_record::Receipts,
    transcript::{History, Transcript},
};
use crate::{
    config::HarnessConfig,
    wire::{
        Command, ConfigOption, LiveTurn, NewEntry, OptionValue, RunId, StoredAttachment, Usage,
    },
};

/// How long a turn's updates are batched before they are published.
const BATCH: Duration = Duration::from_millis(75);
/// How long the agent has to end a cancelled turn before viz gives up on the
/// session.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

/// What [`Session::open`] does with the history `session/load` replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replay {
    /// Restoring a session viz already holds the transcript of.
    Discard,
    /// Importing a native session: its history becomes
    /// [`Session::take_history`].
    Import,
}

/// The session state viz persists and shows, as last reported by the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSnapshot {
    pub(crate) session_id: String,
    pub(crate) options: Vec<ConfigOption>,
    pub(crate) commands: Vec<Command>,
    /// Whether the runner takes image prompts.
    pub(crate) images: bool,
    pub(crate) usage: Option<Usage>,
    /// The runner's own title for the session, unnormalized.
    pub(crate) title: Option<String>,
    /// The values the runner first reported in this process, the catalog's
    /// baseline for unseen options.
    pub(crate) opened: Desired,
}

/// One prompt turn's input.
#[derive(Debug)]
pub(crate) struct Prompt<'a> {
    pub(crate) text: &'a str,
    /// The live turn's identity; text run keys extend it.
    pub(crate) turn: String,
    /// The run the turn belongs to: the scope of the MCP tools it calls.
    pub(crate) run: RunId,
    pub(crate) attachments: &'a [StoredAttachment],
    /// Cancelling sends `session/cancel`; the turn then ends `cancelled`.
    pub(crate) cancel: CancellationToken,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// The agent's message text.
    pub(crate) answer: String,
    /// The ACP stop reason, such as `end_turn` or `cancelled`.
    pub(crate) stop_reason: StopReason,
}

/// Session state the reader task updates as notifications arrive.
#[derive(Default)]
struct State {
    /// Updates for other sessions of the process are ignored once the session
    /// is known.
    session_id: Option<String>,
    options: Vec<ConfigOption>,
    opened: Option<Desired>,
    commands: Vec<Command>,
    usage: Option<Usage>,
    title: Option<String>,
    /// Session-level state changed since the running turn last published.
    dirty: bool,
    sink: Sink,
}

/// Where content updates go.
#[derive(Default)]
enum Sink {
    #[default]
    Discard,
    History(History),
    Turn(mpsc::UnboundedSender<SessionUpdate>),
}

/// A thread's open ACP session. Dropping it stops the harness.
pub(crate) struct Session {
    agent: Agent,
    state: Arc<Mutex<State>>,
    changes: Arc<watch::Sender<()>>,
    id: String,
    config: HarnessConfig,
    /// Opened with `session/new`, so it takes the configured defaults.
    fresh: bool,
    history: Vec<NewEntry>,
    mcp: Option<McpEndpoint>,
    /// A failed or abandoned turn left the session in an unknown state.
    broken: bool,
}

impl Session {
    /// Starts the harness in `cwd` and opens the thread's session:
    /// `session/new` without a `native` session ID, else that session restored
    /// (`Replay::Discard`) or imported (`Replay::Import`, which needs
    /// `session/list` and `session/load`). `mcp` gives the agent viz's tools.
    ///
    /// Dropping the future stops the harness.
    pub(crate) async fn open(
        config: &HarnessConfig,
        cwd: &Path,
        native: Option<&str>,
        mcp: Option<&McpEndpoint>,
        replay: Replay,
    ) -> Result<Self> {
        let state = Arc::new(Mutex::new(State {
            session_id: native.map(str::to_owned),
            sink: if native.is_some() && replay == Replay::Import {
                Sink::History(History::default())
            } else {
                Sink::Discard
            },
            ..State::default()
        }));
        let changes = Arc::new(watch::channel(()).0);
        let agent = Agent::start(config, cwd, update_sink(&state, &changes)).await?;
        let servers = match mcp {
            Some(endpoint) => vec![declaration(&agent, endpoint)?],
            None => Vec::new(),
        };
        let (session_id, options) = match native {
            None => {
                let response = agent.new_session(cwd, servers).await?;
                (response.session_id.0.to_string(), response.config_options)
            }
            Some(id) if replay == Replay::Import => {
                agent.require_import()?;
                (
                    id.to_owned(),
                    agent.load_session(id, cwd, servers).await?.config_options,
                )
            }
            Some(id) => (
                id.to_owned(),
                agent
                    .restore_session(id, cwd, servers)
                    .await?
                    .config_options,
            ),
        };
        let history = {
            let mut state = lock(&state);
            state.session_id = Some(session_id.clone());
            if let Some(options) = options {
                set_options(&mut state, options);
            }
            match mem::take(&mut state.sink) {
                Sink::History(history) => history.finish(),
                Sink::Discard | Sink::Turn(_) => Vec::new(),
            }
        };
        Ok(Self {
            agent,
            state,
            changes,
            id: session_id,
            config: config.clone(),
            fresh: native.is_none(),
            history,
            mcp: mcp.cloned(),
            broken: false,
        })
    }

    /// The runner's ACP session ID, to persist so the thread reopens the same
    /// session.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        let state = lock(&self.state);
        SessionSnapshot {
            session_id: self.id.clone(),
            options: state.options.clone(),
            commands: state.commands.clone(),
            images: self.agent.supports_images(),
            usage: state.usage,
            title: state.title.clone(),
            opened: state.opened.clone().unwrap_or_default(),
        }
    }

    /// Marked changed on every session-level update (options, commands, title,
    /// usage), including while idle.
    pub(crate) fn watch(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }

    /// The imported session's replayed history; empty after the first call and
    /// for other sessions.
    pub(crate) fn take_history(&mut self) -> Vec<NewEntry> {
        mem::take(&mut self.history)
    }

    /// Whether the session can take another turn; drop it and reopen otherwise.
    pub(crate) fn is_alive(&self) -> bool {
        !self.broken && !self.agent.is_closed()
    }

    /// Applies the thread's saved settings after [`Session::open`]: `desired`
    /// values, the configured defaults for a new session, and an exact
    /// `selection`, which fails unless the runner keeps it. Returns the
    /// settings to persist.
    pub(crate) async fn restore(
        &self,
        desired: &Desired,
        selection: Option<&Selection>,
    ) -> Result<Desired> {
        let mut base = if self.fresh {
            defaults(&self.config, &self.options())
        } else {
            Desired::new()
        };
        base.extend(
            desired
                .iter()
                .map(|(id, value)| (id.clone(), value.clone())),
        );
        let model = selection.and_then(|selection| selection.model.as_deref());
        let thinking = selection.and_then(|selection| selection.thinking.as_deref());
        let mut preferred = base.clone();
        // Model changes can change or reset the runner's effort choices, so the
        // model goes first.
        let mut model_id = model_option(&self.options()).map(|option| option.id.clone());
        if let Some(model) = model {
            let id = self.set_exact(model_option, "Model", model).await?;
            preferred.insert(id.clone(), OptionValue::Select(model.to_owned()));
            model_id = Some(id);
        } else if let Some(id) = &model_id
            && let Some(value) = base.get(id)
        {
            self.set_option(id, value).await?;
        }
        if self.fresh {
            let mut with_defaults = defaults(&self.config, &self.options());
            with_defaults.extend(preferred);
            preferred = with_defaults;
        }
        let thinking_id =
            thinking.and_then(|_| thinking_option(&self.options()).map(|option| option.id.clone()));
        for (id, value) in &preferred {
            if Some(id) != model_id.as_ref() && Some(id) != thinking_id.as_ref() {
                self.set_option(id, value).await?;
            }
        }
        if let Some(thinking) = thinking {
            let id = self
                .set_exact(thinking_option, "Thinking level", thinking)
                .await?;
            preferred.insert(id, OptionValue::Select(thinking.to_owned()));
        }
        if let Some(model) = model {
            let retained = model_option(&self.options())
                .is_some_and(|option| option.value == OptionValue::Select(model.to_owned()));
            ensure!(
                retained,
                "{} did not retain model {model:?}.",
                self.agent.label()
            );
        }
        Ok(reconcile(&self.options(), &preferred))
    }

    /// Sets option `id`, reapplying the preferred effort after a model change.
    /// `desired` is the thread's saved settings; returns them updated, to
    /// persist.
    pub(crate) async fn configure(
        &self,
        id: &str,
        value: OptionValue,
        desired: &Desired,
    ) -> Result<Desired> {
        let options = self.options();
        let setting = options
            .iter()
            .find(|option| option.id == id)
            .context("Option not found")?;
        ensure!(valid_value(setting, &value), "Invalid option value");
        let is_model = setting.category.as_deref() == Some("model");
        let mut desired = desired.clone();
        desired.insert(id.to_owned(), value.clone());
        self.set_option(id, &value).await?;
        if is_model
            && let Some(thinking) = thinking_option(&self.options()).map(|option| option.id.clone())
        {
            let preferred = desired
                .get(&thinking)
                .cloned()
                .or_else(|| self.config.effort.clone().map(OptionValue::Select));
            if let Some(preferred) = preferred {
                self.set_option(&thinking, &preferred).await?;
            }
        }
        Ok(reconcile(&self.options(), &desired))
    }

    /// Runs one prompt turn. `updates` receives the entries the turn commits,
    /// in order, with the live tail that follows them and a snapshot when
    /// session state changed since its previous call; calls are serial, batched
    /// about every 75 ms, and the last one has no live tail. It is called for
    /// the end of the turn even after an earlier call failed. ACP prompts have
    /// no idempotency key, so a turn is never retried.
    ///
    /// # Errors
    ///
    /// When the turn fails, the harness closes, or `updates` fails; the session
    /// is then no longer alive.
    pub(crate) async fn prompt<F, Fut>(
        &mut self,
        input: Prompt<'_>,
        mut updates: F,
    ) -> Result<Outcome>
    where
        F: FnMut(Vec<NewEntry>, Option<LiveTurn>, Option<SessionSnapshot>) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        ensure!(self.is_alive(), "the ACP session is closed");
        ensure!(
            !input.cancel.is_cancelled(),
            "the turn was cancelled before it started"
        );
        let blocks = self.prompt_blocks(input.text, input.attachments).await?;
        ensure!(
            !input.cancel.is_cancelled(),
            "the turn was cancelled before it started"
        );
        let receipts = Arc::new(Receipts::default());
        let mut transcript = Transcript::new(input.turn, Arc::clone(&receipts));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        {
            let mut state = lock(&self.state);
            state.sink = Sink::Turn(sender);
            state.dirty = false;
        }
        let tools = self.mcp.as_ref().map(|endpoint| {
            endpoint.activate(
                input.run.0,
                input.cancel.child_token(),
                Arc::clone(&receipts),
            )
        });
        let response = self.agent.prompt(&self.id, &blocks);
        tokio::pin!(response);
        let closed = self.agent.closed();
        tokio::pin!(closed);

        let mut pending = Vec::new();
        let mut publish_at: Option<Instant> = None;
        let mut cancelled = false;
        let mut give_up_at: Option<Instant> = None;
        let mut failure: Option<anyhow::Error> = None;
        let result = loop {
            tokio::select! {
                result = &mut response => break result.map(|response| response.stop_reason),
                reason = &mut closed => break Err(anyhow!("ACP harness closed: {reason}")),
                Some(update) = receiver.recv() => {
                    pending.push(update);
                    publish_at.get_or_insert_with(|| Instant::now() + BATCH);
                }
                () = sleep_until(publish_at.unwrap_or_else(Instant::now)), if publish_at.is_some() => {
                    publish_at = None;
                    let entries = transcript.apply(mem::take(&mut pending));
                    if let Err(error) = updates(entries, Some(transcript.live()), self.take_snapshot()).await {
                        // A failed transcript write invalidates the turn.
                        failure = Some(error);
                        self.agent.cancel(&self.id);
                        break Err(anyhow!("publishing the turn failed"));
                    }
                }
                () = input.cancel.cancelled(), if !cancelled => {
                    cancelled = true;
                    self.agent.cancel(&self.id);
                    give_up_at = Some(Instant::now() + CANCEL_GRACE);
                }
                () = sleep_until(give_up_at.unwrap_or_else(Instant::now)), if give_up_at.is_some() => {
                    break Err(anyhow!("the ACP harness did not end the cancelled turn"));
                }
            }
        };
        drop(tools);
        lock(&self.state).sink = Sink::Discard;
        while let Ok(update) = receiver.try_recv() {
            pending.push(update);
        }
        let mut entries = transcript.apply(pending);
        entries.extend(transcript.finish());
        let published = updates(entries, None, self.take_snapshot()).await;

        let outcome = failure.map_or_else(
            || {
                if cancelled && result.is_err() {
                    Ok(StopReason::Cancelled)
                } else {
                    result
                }
            },
            Err,
        );
        // An abandoned cancellation leaves the agent's turn in an unknown
        // state.
        if outcome.is_err() || give_up_at.is_some_and(|deadline| Instant::now() >= deadline) {
            self.broken = true;
        }
        match (published, outcome) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(_)) => {
                self.broken = true;
                Err(error)
            }
            (Ok(()), Ok(stop_reason)) => Ok(Outcome {
                answer: transcript.answer().to_owned(),
                stop_reason,
            }),
        }
    }

    /// The snapshot for a publish: `Some` when session state changed since the
    /// last one.
    fn take_snapshot(&self) -> Option<SessionSnapshot> {
        let dirty = mem::take(&mut lock(&self.state).dirty);
        dirty.then(|| self.snapshot())
    }

    fn options(&self) -> Vec<ConfigOption> {
        lock(&self.state).options.clone()
    }

    async fn set_option(&self, id: &str, value: &OptionValue) -> Result<()> {
        let unchanged = lock(&self.state)
            .options
            .iter()
            .find(|option| option.id == id)
            .is_none_or(|option| option.value == *value || !valid_value(option, value));
        if unchanged {
            return Ok(());
        }
        let response = self.agent.set_option(&self.id, id, value).await?;
        let mut state = lock(&self.state);
        set_options(&mut state, response.config_options);
        state.dirty = true;
        drop(state);
        self.changes.send_replace(());
        Ok(())
    }

    /// Sets the option `find` locates to exactly `value`, returning its id.
    async fn set_exact(
        &self,
        find: fn(&[ConfigOption]) -> Option<&ConfigOption>,
        label: &str,
        value: &str,
    ) -> Result<String> {
        let wanted = OptionValue::Select(value.to_owned());
        let id = find(&self.options())
            .filter(|option| valid_value(option, &wanted))
            .map(|option| option.id.clone())
            .with_context(|| {
                format!("{label} {value:?} is not available. See list_spawn_options.")
            })?;
        self.set_option(&id, &wanted).await?;
        let options = self.options();
        match find(&options) {
            Some(applied) if applied.value == wanted => Ok(applied.id.clone()),
            _ => bail!(
                "{} did not apply {} {value:?}.",
                self.agent.label(),
                label.to_lowercase()
            ),
        }
    }

    async fn prompt_blocks(
        &self,
        text: &str,
        attachments: &[StoredAttachment],
    ) -> Result<Vec<Value>> {
        let mut blocks =
            vec![json!({ "type": "text", "text": with_attachment_paths(text, attachments) })];
        for attachment in attachments {
            if self.agent.supports_images() && attachment.mime.starts_with("image/") {
                let bytes = tokio::fs::read(&attachment.path)
                    .await
                    .with_context(|| format!("reading attachment {}", attachment.name))?;
                blocks.push(json!({ "type": "image", "data": BASE64_STANDARD.encode(bytes), "mimeType": attachment.mime }));
            }
            let uri = Url::from_file_path(&attachment.path)
                .map_err(|()| anyhow!("attachment {} has no absolute path", attachment.name))?;
            blocks.push(json!({
                "type": "resource_link",
                "uri": uri.as_str(),
                "name": attachment.name,
                "mimeType": attachment.mime,
            }));
        }
        Ok(blocks)
    }
}

/// The wire name of a stop reason, such as `end_turn` or `cancelled`.
pub(crate) fn stop_reason_name(reason: StopReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{reason:?}"))
}

/// The MCP server declaration for the agent: HTTP when it takes HTTP MCP, else
/// the stdio bridge.
fn declaration(agent: &Agent, endpoint: &McpEndpoint) -> Result<Value> {
    if agent.supports_http_mcp() {
        return Ok(json!({
            "type": "http",
            "name": "viz",
            "url": endpoint.url(),
            "headers": [{ "name": "Authorization", "value": format!("Bearer {}", endpoint.token()) }],
        }));
    }
    let executable =
        std::env::current_exe().context("locating the viz-server executable for the MCP bridge")?;
    let executable = executable
        .to_str()
        .context("the viz-server executable path is not valid UTF-8")?;
    Ok(json!({
        "name": "viz",
        "command": executable,
        "args": ["mcp-bridge"],
        "env": [
            { "name": bridge::URL_ENV, "value": endpoint.url() },
            { "name": bridge::TOKEN_ENV, "value": endpoint.token() },
        ],
    }))
}

fn update_sink(state: &Arc<Mutex<State>>, changes: &Arc<watch::Sender<()>>) -> UpdateSink {
    let state = Arc::clone(state);
    let changes = Arc::clone(changes);
    Arc::new(move |notification: SessionNotification| {
        let mut state = lock(&state);
        if state
            .session_id
            .as_deref()
            .is_some_and(|id| *id != *notification.session_id.0)
        {
            return;
        }
        let reported = match notification.update {
            SessionUpdate::ConfigOptionUpdate(update) => {
                set_options(&mut state, update.config_options);
                true
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                state.commands = update.available_commands.into_iter().map(command).collect();
                true
            }
            SessionUpdate::SessionInfoUpdate(update) => match update.title {
                // A cleared title keeps the one shown rather than blank the
                // thread.
                MaybeUndefined::Value(title) => {
                    state.title = Some(title);
                    true
                }
                MaybeUndefined::Undefined | MaybeUndefined::Null => false,
            },
            SessionUpdate::UsageUpdate(update) => {
                state.usage = Some(Usage {
                    used: update.used,
                    size: update.size,
                });
                true
            }
            update => {
                match &mut state.sink {
                    Sink::Discard => {}
                    Sink::History(history) => history.apply(update),
                    Sink::Turn(turn) => {
                        let _ = turn.send(update);
                    }
                }
                false
            }
        };
        if reported {
            state.dirty = true;
            drop(state);
            changes.send_replace(());
        }
    })
}

fn set_options(state: &mut State, options: Vec<SessionConfigOption>) {
    let options = normalize(options);
    state.opened.get_or_insert_with(|| {
        options
            .iter()
            .map(|option| (option.id.clone(), option.value.clone()))
            .collect()
    });
    state.options = options;
}

fn command(command: AvailableCommand) -> Command {
    let hint = match command.input {
        Some(AvailableCommandInput::Unstructured(input)) => Some(input.hint),
        _ => None,
    };
    Command {
        name: command.name,
        description: command.description,
        hint,
    }
}

fn with_attachment_paths(text: &str, attachments: &[StoredAttachment]) -> String {
    if attachments.is_empty() {
        return text.to_owned();
    }
    let mut result = format!("{text}\n\nAttachments (local files):");
    for attachment in attachments {
        let _ = write!(
            result,
            "\n- {}: {}",
            attachment.name,
            attachment.path.display()
        );
    }
    result
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().expect("ACP session state lock poisoned")
}
