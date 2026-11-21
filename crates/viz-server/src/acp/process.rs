//! One ACP harness process and its connection (`docs/findings/acp.md`).
//!
//! viz is the ACP client: it spawns the harness and speaks protocol version 1
//! over stdio as newline-delimited JSON-RPC, with the SDK's typed schema. It
//! advertises no `fs` or `terminal` capabilities, so agents use their own
//! tools, and allows every permission request. One reader handles messages in
//! arrival order, so every `session/update` sent before a response has been
//! delivered by the time that response resolves.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::Path,
    process::Stdio,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use agent_client_protocol::v1::schema::{
    AgentCapabilities, InitializeResponse, LoadSessionResponse, NewSessionResponse,
    PermissionOptionKind, PromptResponse, RequestPermissionRequest, SessionNotification,
    SetSessionConfigOptionResponse,
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{config::HarnessConfig, wire::OptionValue};

const PROTOCOL_VERSION: u16 = 1;
/// How long a harness has to exit after its stdin closes, before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);
const METHOD_NOT_FOUND: i64 = -32601;

static PROCESSES: LazyLock<TaskTracker> = LazyLock::new(TaskTracker::new);

pub(crate) async fn drain_processes() {
    PROCESSES.close();
    if tokio::time::timeout(EXIT_GRACE + Duration::from_secs(1), PROCESSES.wait())
        .await
        .is_err()
    {
        tracing::warn!("ACP processes did not all exit during shutdown");
    }
}

/// Keeps the harness usable without inheriting provider tokens, viz config
/// overrides, or other server secrets.
const RUNTIME_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TZ",
    "TERM",
    "COLORTERM",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// Receives every `session/update`, in arrival order, on the connection's
/// reader task; it must not block.
pub(super) type UpdateSink = Arc<dyn Fn(SessionNotification) + Send + Sync>;

type Waiter = oneshot::Sender<Result<Value, String>>;

struct Connection {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<u64, Waiter>>,
    next_id: AtomicU64,
    closed: CancellationToken,
    reason: Mutex<Option<String>>,
}

impl Connection {
    fn send(&self, message: &Value) {
        // The writer only stops once the process is gone, and then `closed`
        // reports why.
        let _ = self.outgoing.send(message.to_string());
    }

    fn close(&self, reason: String) {
        self.reason
            .lock()
            .expect("ACP close reason lock poisoned")
            .get_or_insert(reason);
        self.closed.cancel();
        let waiters: Vec<Waiter> = self
            .pending
            .lock()
            .expect("ACP pending lock poisoned")
            .drain()
            .map(|(_, waiter)| waiter)
            .collect();
        let reason = self.reason();
        for waiter in waiters {
            let _ = waiter.send(Err(reason.clone()));
        }
    }

    fn reason(&self) -> String {
        self.reason
            .lock()
            .expect("ACP close reason lock poisoned")
            .clone()
            .unwrap_or_default()
    }

    async fn request<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (waiter, response) = oneshot::channel();
        self.pending
            .lock()
            .expect("ACP pending lock poisoned")
            .insert(id, waiter);
        if self.closed.is_cancelled() {
            self.pending
                .lock()
                .expect("ACP pending lock poisoned")
                .remove(&id);
            bail!("ACP harness closed: {}", self.reason());
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let result = response
            .await
            .map_err(|_| anyhow!("ACP harness closed: {}", self.reason()))?
            .map_err(|message| anyhow!("{method}: {message}"))?;
        serde_json::from_value(result)
            .with_context(|| format!("decoding the ACP {method} response"))
    }
}

/// Cancelled when the [`Agent`] drops: closes the harness's stdin, its cue to
/// exit, and kills it after a grace period.
struct Shutdown(CancellationToken);

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A running, initialized ACP harness. Dropping it stops the process.
pub(super) struct Agent {
    connection: Arc<Connection>,
    capabilities: AgentCapabilities,
    label: String,
    _shutdown: Shutdown,
}

impl Agent {
    /// Spawns the harness in `cwd` with the runtime environment and its
    /// configured `env`, and initializes the connection.
    pub(super) async fn start(
        config: &HarnessConfig,
        cwd: &Path,
        updates: UpdateSink,
    ) -> Result<Self> {
        let label = config.command.clone();
        let mut child = Command::new(&config.command)
            .args(&config.args)
            .current_dir(cwd)
            .env_clear()
            .envs(launch_env(config)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting ACP harness {label}"))?;
        let stdin = child.stdin.take().context("harness stdin is not piped")?;
        let stdout = child.stdout.take().context("harness stdout is not piped")?;
        let stderr = child.stderr.take().context("harness stderr is not piped")?;
        let (outgoing, lines) = mpsc::unbounded_channel();
        let connection = Arc::new(Connection {
            outgoing,
            pending: Mutex::default(),
            next_id: AtomicU64::new(1),
            closed: CancellationToken::new(),
            reason: Mutex::default(),
        });
        let shutdown = Shutdown(CancellationToken::new());
        tokio::spawn(log(stderr, label.clone()));
        tokio::spawn(write(stdin, lines, shutdown.0.clone()));
        tokio::spawn(read(stdout, Arc::clone(&connection), updates));
        PROCESSES.spawn(supervise(
            child,
            Arc::clone(&connection),
            shutdown.0.clone(),
            label.clone(),
        ));

        let init: InitializeResponse = connection
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "viz", "title": "viz", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await
            .with_context(|| format!("initializing ACP harness {label}"))?;
        ensure!(
            init.protocol_version.as_u16() == PROTOCOL_VERSION,
            "{label} uses unsupported ACP protocol version {}",
            init.protocol_version.as_u16()
        );
        Ok(Self {
            connection,
            capabilities: init.agent_capabilities,
            label,
            _shutdown: shutdown,
        })
    }

    /// The harness command, for messages.
    pub(super) fn label(&self) -> &str {
        &self.label
    }

    /// Whether the agent takes viz's tools over HTTP MCP; otherwise they go
    /// through the stdio bridge.
    pub(super) const fn supports_http_mcp(&self) -> bool {
        self.capabilities.mcp_capabilities.http
    }

    pub(super) const fn supports_images(&self) -> bool {
        self.capabilities.prompt_capabilities.image
    }

    pub(super) fn is_closed(&self) -> bool {
        self.connection.closed.is_cancelled()
    }

    /// Resolves when the connection closes, with the reason.
    pub(super) fn closed(&self) -> impl Future<Output = String> + Send + 'static {
        let connection = Arc::clone(&self.connection);
        async move {
            connection.closed.cancelled().await;
            connection.reason()
        }
    }

    pub(super) async fn new_session(
        &self,
        cwd: &Path,
        mcp: Vec<Value>,
    ) -> Result<NewSessionResponse> {
        let cwd = utf8(cwd)?;
        self.connection
            .request("session/new", json!({ "cwd": cwd, "mcpServers": mcp }))
            .await
    }

    /// Reopens a session: `session/resume` when offered (no history), else
    /// `session/load`, whose replay the caller discards.
    pub(super) async fn restore_session(
        &self,
        id: &str,
        cwd: &Path,
        mcp: Vec<Value>,
    ) -> Result<LoadSessionResponse> {
        let params = json!({ "sessionId": id, "cwd": utf8(cwd)?, "mcpServers": mcp });
        if self.capabilities.session_capabilities.resume.is_some() {
            // A resume response has the load response's fields.
            return self.connection.request("session/resume", params).await;
        }
        ensure!(
            self.capabilities.load_session,
            "ACP harness {} cannot restore sessions",
            self.label
        );
        self.connection.request("session/load", params).await
    }

    /// Deletes a disposable session where the agent can, else closes it.
    pub(super) async fn discard_session(&self, id: &str) -> Result<()> {
        let capabilities = &self.capabilities.session_capabilities;
        let method = if capabilities.delete.is_some() {
            "session/delete"
        } else if capabilities.close.is_some() {
            "session/close"
        } else {
            return Ok(());
        };
        self.connection
            .request::<Value>(method, json!({ "sessionId": id }))
            .await
            .map(drop)
    }

    pub(super) async fn set_option(
        &self,
        session: &str,
        id: &str,
        value: &OptionValue,
    ) -> Result<SetSessionConfigOptionResponse> {
        let params = match value {
            OptionValue::Boolean(value) => {
                json!({ "sessionId": session, "configId": id, "type": "boolean", "value": value })
            }
            OptionValue::Select(value) => {
                json!({ "sessionId": session, "configId": id, "value": value })
            }
        };
        self.connection
            .request("session/set_config_option", params)
            .await
    }

    /// Sends one prompt; the returned future resolves when the agent ends the
    /// turn. It does not borrow the agent, so the caller can keep using it
    /// while the turn runs.
    pub(super) fn prompt(
        &self,
        session: &str,
        prompt: &[Value],
    ) -> impl Future<Output = Result<PromptResponse>> + Send + 'static {
        let connection = Arc::clone(&self.connection);
        let params = json!({ "sessionId": session, "prompt": prompt });
        async move { connection.request("session/prompt", params).await }
    }

    /// Asks the agent to end its current turn; the prompt then stops with
    /// `cancelled`.
    pub(super) fn cancel(&self, session: &str) {
        self.connection.send(&json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }));
    }
}

/// ACP paths are JSON strings; serializing a non-UTF-8 path would fail.
fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

fn launch_env(config: &HarnessConfig) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for (name, secret) in &config.env {
        let value = secret
            .resolve()
            .map_err(|error| anyhow!("resolving harness env {name}: {error}"))?;
        env.insert(name.clone(), value);
    }
    for name in RUNTIME_ENV {
        if !env.contains_key(*name)
            && let Ok(value) = std::env::var(name)
        {
            env.insert((*name).to_owned(), value);
        }
    }
    Ok(env)
}

async fn write(
    mut stdin: ChildStdin,
    mut lines: mpsc::UnboundedReceiver<String>,
    shutdown: CancellationToken,
) {
    loop {
        let line = tokio::select! {
            line = lines.recv() => line,
            () = shutdown.cancelled() => None,
        };
        let Some(line) = line else { break };
        if write_line(&mut stdin, &line).await.is_err() {
            return;
        }
    }
    // Deliver what was sent before the shutdown, such as a final
    // `session/cancel`; dropping stdin then closes it.
    while let Ok(line) = lines.try_recv() {
        if write_line(&mut stdin, &line).await.is_err() {
            return;
        }
    }
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    stdin.write_all(line.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await
}

async fn read(stdout: ChildStdout, connection: Arc<Connection>, updates: UpdateSink) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => {}
            Ok(Some(line)) => dispatch(&connection, &updates, &line),
            Ok(None) => break,
            Err(error) => {
                connection.close(format!("reading from the harness: {error}"));
                return;
            }
        }
    }
    connection.close("the harness closed its output".to_owned());
}

fn dispatch(connection: &Connection, updates: &UpdateSink, line: &str) {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            tracing::warn!("ACP: ignoring a malformed message: {error}");
            return;
        }
    };
    let id = message.get("id").cloned();
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match id {
            Some(id) => connection.send(&answer(method, params, &id)),
            None if method == "session/update" => {
                match serde_json::from_value::<SessionNotification>(params) {
                    Ok(notification) => updates(notification),
                    Err(error) => {
                        tracing::warn!("ACP: ignoring a malformed session/update: {error}");
                    }
                }
            }
            None => {}
        }
        return;
    }
    let Some(id) = id.as_ref().and_then(Value::as_u64) else {
        return;
    };
    let waiter = connection
        .pending
        .lock()
        .expect("ACP pending lock poisoned")
        .remove(&id);
    let Some(waiter) = waiter else { return };
    let result = message.get("error").map_or_else(
        || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        |error| {
            Err(error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
                .to_owned())
        },
    );
    let _ = waiter.send(result);
}

/// The response to an agent's request: permission requests are allowed,
/// everything else is unsupported.
fn answer(method: &str, params: Value, id: &Value) -> Value {
    if method == "session/request_permission"
        && let Ok(request) = serde_json::from_value::<RequestPermissionRequest>(params)
    {
        return json!({ "jsonrpc": "2.0", "id": id, "result": allow(&request) });
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": METHOD_NOT_FOUND, "message": format!("viz does not support {method}") },
    })
}

/// Allows once if offered, else always; a request without an allow option is
/// cancelled.
fn allow(request: &RequestPermissionRequest) -> Value {
    let option = [
        PermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways,
    ]
    .iter()
    .find_map(|kind| request.options.iter().find(|option| option.kind == *kind));
    option.map_or_else(
        || json!({ "outcome": { "outcome": "cancelled" } }),
        |option| json!({ "outcome": { "outcome": "selected", "optionId": option.option_id } }),
    )
}

async fn supervise(
    mut child: Child,
    connection: Arc<Connection>,
    shutdown: CancellationToken,
    label: String,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        () = shutdown.cancelled() => {
            if let Ok(status) = tokio::time::timeout(EXIT_GRACE, child.wait()).await {
                status
            } else if let Err(error) = child.start_kill() {
                Err(error)
            } else {
                child.wait().await
            }
        },
    };
    connection.close(match status {
        Ok(status) => format!("{label} exited with {status}"),
        Err(error) => format!("{label} failed: {error}"),
    });
}

async fn log(stream: impl AsyncRead + Unpin, label: String) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            tracing::info!("ACP {label}: {line}");
        }
    }
}
