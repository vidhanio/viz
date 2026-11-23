//! Runs agent-requested terminals and keeps their recent output.

use std::{
    collections::VecDeque,
    io,
    os::unix::process::ExitStatusExt as _,
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::Duration,
};

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Child,
    sync::{mpsc, watch},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ClientEvent,
    schema::{SessionId, TerminalExitStatus, TerminalId, TerminalOutputResponse},
};

const TERMINAL_HISTORY: usize = 16;
const DRAIN_GRACE: Duration = Duration::from_secs(1);

pub(super) struct Terminal {
    pub(super) session: SessionId,
    pub(super) output: Arc<Mutex<Output>>,
    pub(super) status: watch::Receiver<Option<TerminalExitStatus>>,
    pub(super) cancel: CancellationToken,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(super) struct TerminalHistory {
    pub(super) recent: Mutex<VecDeque<(SessionId, TerminalId, TerminalOutputResponse)>>,
    pub(super) events: Option<mpsc::UnboundedSender<ClientEvent>>,
}

pub(super) struct Completion {
    pub(super) history: Arc<TerminalHistory>,
    pub(super) session: SessionId,
    pub(super) id: TerminalId,
}

impl Completion {
    pub(super) fn publish(self, output: TerminalOutputResponse) {
        let mut recent = self
            .history
            .recent
            .lock()
            .expect("ACP terminal history lock poisoned");
        recent.push_back((self.session.clone(), self.id.clone(), output.clone()));
        if recent.len() > TERMINAL_HISTORY {
            recent.pop_front();
        }
        drop(recent);
        if let Some(events) = &self.history.events {
            let _ = events.send(ClientEvent::Terminal {
                session: self.session,
                terminal: self.id,
                output: Box::new(output),
            });
        }
    }
}

pub(super) fn terminal_snapshot(
    output: &Mutex<Output>,
    status: Option<TerminalExitStatus>,
) -> TerminalOutputResponse {
    let output = output.lock().expect("ACP terminal output lock poisoned");
    let text = String::from_utf8_lossy(&output.bytes);
    let start = text.ceil_char_boundary(text.len().saturating_sub(output.limit));
    TerminalOutputResponse::new(&text[start..], output.truncated || start != 0).exit_status(status)
}

pub(super) struct Output {
    pub(super) bytes: Vec<u8>,
    pub(super) limit: usize,
    pub(super) truncated: bool,
}

async fn capture(reader: Option<impl AsyncRead + Unpin>, output: &Mutex<Output>) {
    let Some(mut stream) = reader else { return };
    let mut buf = [0; 8192];
    while let Ok(count @ 1..) = stream.read(&mut buf).await {
        let mut output = output.lock().expect("ACP terminal output lock poisoned");
        output.bytes.extend_from_slice(&buf[..count]);
        let excess = output.bytes.len().saturating_sub(output.limit);
        if excess > 0 {
            let remove = output.bytes[excess..]
                .iter()
                .position(|byte| byte & 0xc0 != 0x80)
                .map_or(output.bytes.len(), |offset| excess + offset);
            output.bytes.drain(..remove);
            output.truncated = true;
        }
    }
}

pub(super) async fn run_terminal(
    mut child: Child,
    output: Arc<Mutex<Output>>,
    status: watch::Sender<Option<TerminalExitStatus>>,
    cancel: CancellationToken,
    completion: Completion,
) {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let streams = async {
        tokio::join!(capture(stdout, &output), capture(stderr, &output));
    };
    tokio::pin!(streams);
    let exited = tokio::select! {
        result = wait_child(&mut child, cancel.clone()) => {
            let _ = tokio::time::timeout(DRAIN_GRACE, &mut streams).await;
            result
        }
        () = &mut streams => wait_child(&mut child, cancel).await,
    };
    if let Ok(exited) = exited {
        let code = exited.code().and_then(|code| u32::try_from(code).ok());
        let signal = exited.signal().map(|signal| signal.to_string());
        let exit = TerminalExitStatus::new().exit_code(code).signal(signal);
        completion.publish(terminal_snapshot(&output, Some(exit.clone())));
        status.send_replace(Some(exit));
    }
}

async fn wait_child(child: &mut Child, cancel: CancellationToken) -> io::Result<ExitStatus> {
    tokio::select! {
        status = child.wait() => status,
        () = cancel.cancelled() => {
            kill_group(child);
            child.wait().await
        },
    }
}

pub fn kill_group(child: &mut Child) {
    if let Some(pid) = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw)
    {
        let _ = kill_process_group(pid, Signal::KILL);
    }
    let _ = child.start_kill();
}
