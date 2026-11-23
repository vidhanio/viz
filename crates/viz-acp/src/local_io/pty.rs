//! ACP terminals as PTYs in the environment's shared terminal registry, where
//! clients can attach to the same terminal the agent runs a command in.
//!
//! The agent reads plain text: escape sequences are removed and line endings
//! normalised, while attached viewers see the PTY as it is. A monitor task
//! keeps a readable copy of each terminal's output, so tool calls can embed
//! it without waiting on the PTY.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use viz_protocol::ThreadId;
use viz_terminal::{
    CellSize, Exit, Size,
    pty::{Launch, Program, TerminalHandle, Terminals},
};

use super::terminals::{Completion, TerminalHistory};
use crate::schema::{
    CreateTerminalRequest, SessionId, TerminalExitStatus, TerminalId, TerminalOutputResponse,
};

const REFRESH: Duration = Duration::from_millis(200);
/// Wide enough that most command output does not wrap.
const COLUMNS: u16 = 160;
const ROWS: u16 = 48;

/// The environment's terminal registry, and the thread whose agent creates
/// terminals in it.
#[derive(Clone)]
pub struct PtyTerminals {
    registry: Arc<AsyncMutex<Terminals>>,
    thread: ThreadId,
}

impl PtyTerminals {
    #[must_use]
    pub const fn new(registry: Arc<AsyncMutex<Terminals>>, thread: ThreadId) -> Self {
        Self { registry, thread }
    }
}

impl std::fmt::Debug for PtyTerminals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyTerminals")
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

struct PtyTerminal {
    session: SessionId,
    handle: TerminalHandle,
    view: Arc<Mutex<TerminalOutputResponse>>,
}

pub(super) struct Ptys {
    owner: PtyTerminals,
    open: Mutex<HashMap<TerminalId, PtyTerminal>>,
}

impl Ptys {
    pub(super) fn new(owner: PtyTerminals) -> Self {
        Self {
            owner,
            open: Mutex::default(),
        }
    }

    fn open(&self) -> std::sync::MutexGuard<'_, HashMap<TerminalId, PtyTerminal>> {
        self.open.lock().expect("ACP PTY lock poisoned")
    }

    pub(super) async fn create(
        &self,
        request: CreateTerminalRequest,
        cwd: PathBuf,
        limit: usize,
        history: Arc<TerminalHistory>,
        shutdown: &CancellationToken,
    ) -> Result<TerminalId, viz_terminal::pty::PtyError> {
        let launch = Launch {
            program: Program::Command {
                executable: request.command.into(),
                args: request.args.into_iter().map(OsString::from).collect(),
            },
            cwd,
            env: request
                .env
                .into_iter()
                .map(|entry| (entry.name.into(), entry.value.into()))
                .collect(),
            size: Size::new(COLUMNS, ROWS),
            cell: CellSize::default(),
        };
        let handle = self
            .owner
            .registry
            .lock()
            .await
            .create(self.owner.thread, launch)
            .await?;
        let id = TerminalId::new(handle.id().0.to_string());
        let view = Arc::new(Mutex::new(TerminalOutputResponse::new(
            String::new(),
            false,
        )));
        self.open().insert(
            id.clone(),
            PtyTerminal {
                session: request.session_id.clone(),
                handle: handle.clone(),
                view: Arc::clone(&view),
            },
        );
        let completion = Completion {
            history,
            session: request.session_id,
            id: id.clone(),
        };
        tokio::spawn(monitor(handle, view, limit, completion, shutdown.clone()));
        Ok(id)
    }

    fn find(
        &self,
        session: &SessionId,
        id: &TerminalId,
    ) -> Option<(TerminalHandle, Arc<Mutex<TerminalOutputResponse>>)> {
        self.open()
            .get(id)
            .filter(|terminal| &terminal.session == session)
            .map(|terminal| (terminal.handle.clone(), Arc::clone(&terminal.view)))
    }

    pub(super) fn output(
        &self,
        session: &SessionId,
        id: &TerminalId,
    ) -> Option<TerminalOutputResponse> {
        self.find(session, id)
            .map(|(_, view)| view.lock().expect("ACP PTY view poisoned").clone())
    }

    pub(super) async fn wait(
        &self,
        session: &SessionId,
        id: &TerminalId,
    ) -> Option<TerminalExitStatus> {
        let (handle, _) = self.find(session, id)?;
        handle.wait_for_exit().await.ok().map(exit_status)
    }

    pub(super) async fn kill(&self, session: &SessionId, id: &TerminalId) -> bool {
        let Some((handle, _)) = self.find(session, id) else {
            return false;
        };
        let _ = handle.kill().await;
        true
    }

    pub(super) async fn release(&self, session: &SessionId, id: &TerminalId) -> bool {
        if self.find(session, id).is_none() {
            return false;
        }
        let Some(terminal) = self.open().remove(id) else {
            return false;
        };
        self.owner
            .registry
            .lock()
            .await
            .release(self.owner.thread, terminal.handle.id());
        true
    }

    /// Kills the commands a session started, for its cancelled turn.
    pub(super) fn kill_session(&self, session: &str) {
        let handles: Vec<_> = self
            .open()
            .values()
            .filter(|terminal| terminal.session.0.as_ref() == session)
            .map(|terminal| terminal.handle.clone())
            .collect();
        tokio::spawn(async move {
            for handle in handles {
                let _ = handle.kill().await;
            }
        });
    }

    /// Releases every terminal the harness left open.
    pub(super) fn shutdown(&self) {
        let ids: Vec<_> = self
            .open()
            .drain()
            .map(|(_, terminal)| terminal.handle.id())
            .collect();
        if ids.is_empty() {
            return;
        }
        let owner = self.owner.clone();
        tokio::spawn(async move {
            let mut registry = owner.registry.lock().await;
            for id in ids {
                registry.release(owner.thread, id);
            }
        });
    }
}

async fn monitor(
    handle: TerminalHandle,
    view: Arc<Mutex<TerminalOutputResponse>>,
    limit: usize,
    completion: Completion,
    shutdown: CancellationToken,
) {
    let refresh = async |exit: Option<Exit>| -> Option<TerminalOutputResponse> {
        let captured = handle.output(limit).await.ok()?;
        let text = plain_text(&captured.text);
        let start = text.ceil_char_boundary(text.len().saturating_sub(limit));
        let output = TerminalOutputResponse::new(&text[start..], captured.truncated || start > 0)
            .exit_status(exit.or(captured.exit).map(exit_status));
        *view.lock().expect("ACP PTY view poisoned") = output.clone();
        Some(output)
    };
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            exit = handle.wait_for_exit() => {
                if let Some(output) = refresh(exit.ok()).await {
                    completion.publish(output);
                }
                return;
            }
            () = tokio::time::sleep(REFRESH) => {
                if refresh(None).await.is_none() {
                    return;
                }
            }
        }
    }
}

fn exit_status(exit: Exit) -> TerminalExitStatus {
    match exit {
        Exit::Code(code) => TerminalExitStatus::new().exit_code(u32::try_from(code).ok()),
        Exit::Signal(signal) => TerminalExitStatus::new().signal(Some(signal.to_string())),
        Exit::Lost => TerminalExitStatus::new(),
    }
}

/// Removes ANSI escape sequences and carriage returns that end lines, as a
/// pipe would have shown the agent.
fn plain_text(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}'
                            || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some())
                        {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            ch => text.push(ch),
        }
    }
    text
}
