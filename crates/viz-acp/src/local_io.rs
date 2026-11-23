//! Opt-in ACP filesystem and process operations on the client's host.

mod files;
mod pty;
mod terminals;

use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{Error, JsonRpcResponse as _};
pub use pty::PtyTerminals;
use serde_json::Value;
pub use terminals::kill_group;
use tokio::{
    process::Command,
    sync::{mpsc, watch},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use self::{
    files::{read_text_file, write_text_file},
    pty::Ptys,
    terminals::{Completion, Output, Terminal, TerminalHistory, run_terminal, terminal_snapshot},
};
use crate::{
    ClientEvent,
    process::{PROCESSES, runtime_env},
    schema::{
        AgentRequest, CreateTerminalRequest, CreateTerminalResponse, FileSystemCapabilities,
        KillTerminalResponse, ReadTextFileResponse, ReleaseTerminalResponse, SessionId, TerminalId,
        TerminalOutputResponse, WaitForTerminalExitResponse, WriteTextFileResponse,
    },
};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const OUTPUT_DEFAULT: u64 = 64 * 1024;

/// File access exposed over ACP. Files must be within an explicitly granted
/// root.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FileAccess {
    #[default]
    Disabled,
    ReadOnly,
    ReadWrite,
}

/// Whether to execute agent-supplied commands on the client's host.
#[derive(Debug, Default, Clone)]
pub enum TerminalAccess {
    #[default]
    Disabled,
    /// Runs commands with the client's OS permissions and a scrubbed inherited
    /// environment.
    Local,
    /// Runs commands in PTYs of the environment's terminal registry, where
    /// clients can watch them.
    Pty(PtyTerminals),
}

impl TerminalAccess {
    const fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// Configuration for optional host-side ACP filesystem and terminal operations.
#[derive(Debug, Default, Clone)]
pub struct LocalIo {
    /// Absolute roots. The harness working directory is used when empty.
    pub roots: Vec<PathBuf>,
    pub files: FileAccess,
    pub terminals: TerminalAccess,
}

impl LocalIo {
    #[must_use]
    pub fn file_capabilities(&self) -> FileSystemCapabilities {
        FileSystemCapabilities::new()
            .read_text_file(self.files != FileAccess::Disabled)
            .write_text_file(self.files == FileAccess::ReadWrite)
    }
}

pub struct Host {
    policy: LocalIo,
    cwd: PathBuf,
    roots: Vec<PathBuf>,
    terminals: Mutex<HashMap<TerminalId, Terminal>>,
    ptys: Option<Ptys>,
    history: Arc<TerminalHistory>,
    shutdown: CancellationToken,
}

impl Host {
    pub async fn new(
        policy: LocalIo,
        cwd: &Path,
        events: Option<mpsc::UnboundedSender<ClientEvent>>,
    ) -> crate::error::Result<Self> {
        let mut roots = Vec::new();
        if policy.files != FileAccess::Disabled || policy.terminals.enabled() {
            let configured = if policy.roots.is_empty() {
                std::slice::from_ref(&cwd)
            } else {
                &[]
            };
            for path in configured
                .iter()
                .copied()
                .chain(policy.roots.iter().map(PathBuf::as_path))
            {
                if !path.is_absolute() {
                    return Err(crate::Error::Path(path.to_owned()));
                }
                let root =
                    tokio::fs::canonicalize(path)
                        .await
                        .map_err(|source| crate::Error::Io {
                            operation: "resolving an ACP client root",
                            source,
                        })?;
                if !root.is_dir() {
                    return Err(crate::Error::Path(root));
                }
                roots.push(root);
            }
        }
        let ptys = match &policy.terminals {
            TerminalAccess::Pty(owner) => Some(Ptys::new(owner.clone())),
            TerminalAccess::Disabled | TerminalAccess::Local => None,
        };
        Ok(Self {
            policy,
            cwd: cwd.to_owned(),
            roots,
            terminals: Mutex::default(),
            ptys,
            shutdown: CancellationToken::new(),
            history: Arc::new(TerminalHistory {
                recent: Mutex::default(),
                events,
            }),
        })
    }

    pub async fn handle(&self, request: &AgentRequest, method: &str) -> Result<Value, Error> {
        match request {
            AgentRequest::ReadTextFileRequest(request)
                if self.policy.files != FileAccess::Disabled =>
            {
                let (root, path) = self.relative(&request.path)?;
                let request = request.clone();
                let text = blocking(move || read_text_file(&root, &path, &request)).await?;
                ReadTextFileResponse::new(text).into_json(method)
            }
            AgentRequest::WriteTextFileRequest(request)
                if self.policy.files == FileAccess::ReadWrite =>
            {
                let (root, path) = self.relative(&request.path)?;
                let content = request.content.clone();
                blocking(move || write_text_file(&root, &path, &content)).await?;
                WriteTextFileResponse::new().into_json(method)
            }
            AgentRequest::CreateTerminalRequest(request) if self.policy.terminals.enabled() => {
                CreateTerminalResponse::new(self.create(request.clone()).await?).into_json(method)
            }
            AgentRequest::TerminalOutputRequest(request) => self
                .output(&request.session_id, &request.terminal_id)
                .ok_or_else(unknown_terminal)?
                .into_json(method),
            AgentRequest::WaitForTerminalExitRequest(request) if let Some(ptys) = &self.ptys => {
                let status = ptys
                    .wait(&request.session_id, &request.terminal_id)
                    .await
                    .ok_or_else(unknown_terminal)?;
                WaitForTerminalExitResponse::new(status).into_json(method)
            }
            AgentRequest::KillTerminalRequest(request) if let Some(ptys) = &self.ptys => {
                if !ptys.kill(&request.session_id, &request.terminal_id).await {
                    return Err(unknown_terminal());
                }
                KillTerminalResponse::new().into_json(method)
            }
            AgentRequest::ReleaseTerminalRequest(request) if let Some(ptys) = &self.ptys => {
                if !ptys
                    .release(&request.session_id, &request.terminal_id)
                    .await
                {
                    return Err(unknown_terminal());
                }
                ReleaseTerminalResponse::new().into_json(method)
            }
            AgentRequest::WaitForTerminalExitRequest(request) => {
                let mut status =
                    self.terminal(&request.session_id, &request.terminal_id, |terminal| {
                        terminal.status.clone()
                    })?;
                let status = status
                    .wait_for(Option::is_some)
                    .await
                    .map_err(Error::into_internal_error)?
                    .clone();
                WaitForTerminalExitResponse::new(status.ok_or_else(unknown_terminal)?)
                    .into_json(method)
            }
            AgentRequest::KillTerminalRequest(request) => {
                self.terminal(&request.session_id, &request.terminal_id, |terminal| {
                    terminal.cancel.cancel();
                })?;
                KillTerminalResponse::new().into_json(method)
            }
            AgentRequest::ReleaseTerminalRequest(request) => {
                let mut terminals = self.terminals.lock().expect("ACP terminals lock poisoned");
                let terminal = terminals
                    .get(&request.terminal_id)
                    .ok_or_else(unknown_terminal)?;
                if terminal.session != request.session_id {
                    return Err(unknown_terminal());
                }
                terminals.remove(&request.terminal_id);
                drop(terminals);
                ReleaseTerminalResponse::new().into_json(method)
            }
            _ => Err(Error::method_not_found()),
        }
    }

    fn relative(&self, path: &Path) -> Result<(PathBuf, PathBuf), Error> {
        self.roots
            .iter()
            .find_map(|root| {
                path.strip_prefix(root)
                    .ok()
                    .map(|relative| (root.clone(), relative.to_owned()))
            })
            .filter(|(_, relative)| {
                relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            })
            .ok_or_else(|| {
                Error::invalid_params().data("path must be inside a client workspace root")
            })
    }

    async fn create(&self, request: CreateTerminalRequest) -> Result<TerminalId, Error> {
        let cwd = request.cwd.as_deref().unwrap_or(&self.cwd);
        let cwd = tokio::fs::canonicalize(cwd)
            .await
            .map_err(Error::into_internal_error)?;
        self.relative(&cwd)?;
        let limit = request.output_byte_limit.unwrap_or(OUTPUT_DEFAULT);
        let limit = usize::try_from(limit)
            .unwrap_or(OUTPUT_LIMIT)
            .min(OUTPUT_LIMIT);
        if let Some(ptys) = &self.ptys {
            return ptys
                .create(
                    request,
                    cwd,
                    limit,
                    Arc::clone(&self.history),
                    &self.shutdown,
                )
                .await
                .map_err(Error::into_internal_error);
        }
        let child = Command::new(&request.command)
            .args(request.args)
            .current_dir(cwd)
            .env_clear()
            .envs(runtime_env())
            .envs(
                request
                    .env
                    .into_iter()
                    .map(|entry| (entry.name, entry.value)),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(Error::into_internal_error)?;
        let cancel = self.shutdown.child_token();
        let output = Arc::new(Mutex::new(Output {
            limit,
            bytes: Vec::new(),
            truncated: false,
        }));
        let (status_sender, status) = watch::channel(None);
        let id = TerminalId::new(Uuid::new_v4().simple().to_string());
        let session = request.session_id;
        let terminal = Terminal {
            session: session.clone(),
            output: Arc::clone(&output),
            status,
            cancel: cancel.clone(),
        };
        self.terminals
            .lock()
            .expect("ACP terminals lock poisoned")
            .insert(id.clone(), terminal);
        PROCESSES.spawn(run_terminal(
            child,
            output,
            status_sender,
            cancel,
            Completion {
                history: Arc::clone(&self.history),
                session,
                id: id.clone(),
            },
        ));
        Ok(id)
    }

    fn terminal<T>(
        &self,
        session: &SessionId,
        id: &TerminalId,
        use_terminal: impl FnOnce(&Terminal) -> T,
    ) -> Result<T, Error> {
        let terminals = self.terminals.lock().expect("ACP terminals lock poisoned");
        let terminal = terminals
            .get(id)
            .filter(|terminal| &terminal.session == session)
            .ok_or_else(unknown_terminal)?;
        let value = use_terminal(terminal);
        drop(terminals);
        Ok(value)
    }

    fn output(&self, session: &SessionId, id: &TerminalId) -> Option<TerminalOutputResponse> {
        if let Some(output) = self.ptys.as_ref().and_then(|ptys| ptys.output(session, id)) {
            return Some(output);
        }
        self.terminal(session, id, |terminal| {
            let status = terminal.status.borrow().clone();
            terminal_snapshot(&terminal.output, status)
        })
        .ok()
    }

    pub fn tool_output(
        &self,
        session: &SessionId,
        id: &TerminalId,
    ) -> Option<TerminalOutputResponse> {
        self.output(session, id).or_else(|| {
            let recent = self
                .history
                .recent
                .lock()
                .expect("ACP terminal history lock poisoned");
            recent
                .iter()
                .find(|(owner, terminal, _)| owner == session && terminal == id)
                .map(|(_, _, output)| output.clone())
        })
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        if let Some(ptys) = &self.ptys {
            ptys.shutdown();
        }
    }

    pub fn cancel_session(&self, session: &str) {
        if let Some(ptys) = &self.ptys {
            ptys.kill_session(session);
        }
        for terminal in self
            .terminals
            .lock()
            .expect("ACP terminals lock poisoned")
            .values()
        {
            if terminal.session.0.as_ref() == session {
                terminal.cancel.cancel();
            }
        }
    }
}

fn unknown_terminal() -> Error {
    Error::invalid_params().data("unknown terminal for this session")
}

async fn blocking<T: Send + 'static>(
    action: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(action)
        .await
        .map_err(Error::into_internal_error)?
        .map_err(Error::into_internal_error)
}
