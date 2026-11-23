//! PTY ownership, bounded capture, and attachment boundaries.

use std::{
    collections::{HashMap, VecDeque},
    env,
    ffi::OsString,
    io,
    os::unix::process::ExitStatusExt as _,
    path::PathBuf,
    sync::{Arc, Weak},
    thread,
    time::Duration,
};

use pty_process::{Command, Pty};
use rustix::{
    process::{Pid, Signal, getsid, kill_process_group},
    termios::tcgetpgrp,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::{broadcast, mpsc, oneshot, watch},
    time::{self, Instant},
};
use viz_protocol::ThreadId;

use crate::{CellSize, EmulationError, Exit, LiveScreen, Size, Snapshot, TerminalId, Update};

const MAX_SESSIONS: usize = 32;
const MAX_INPUT: usize = 64 * 1024;
const MAX_BATCH: usize = 64 * 1024;
const READ_BYTES: usize = 16 * 1024;
const MAX_CAPTURE: usize = 1024 * 1024;
const BATCH_DELAY: Duration = Duration::from_millis(12);
const DRAIN_DELAY: Duration = Duration::from_millis(250);
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

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

/// Terminal creation or operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    /// PTY I/O failed.
    #[error("terminal i/o failed")]
    Io(#[from] io::Error),
    /// The host failed a PTY operation.
    #[error("pty operation failed")]
    Pty(#[from] pty_process::Error),
    /// The interpreter could not complete an operation.
    #[error(transparent)]
    Screen(#[from] EmulationError),
    /// All of the environment's session slots are in use.
    #[error("terminal session limit reached")]
    Capacity,
    /// The process or session has closed.
    #[error("terminal is closed")]
    Closed,
    /// The requested size conflicts with another viewer's resize lease.
    #[error("another viewer owns terminal resizing")]
    ResizeLease,
    /// The input request exceeded the per-command byte limit.
    #[error("terminal input exceeds the byte limit")]
    InputTooLarge,
}

/// Terminal command and initial environment.
#[derive(Debug)]
pub struct Launch {
    /// Login shell or ACP command.
    pub program: Program,
    /// Directory in the owning environment.
    pub cwd: PathBuf,
    /// Additional variables explicitly supplied for the command.
    pub env: Vec<(OsString, OsString)>,
    /// Initial grid dimensions.
    pub size: Size,
    /// Initial physical cell dimensions.
    pub cell: CellSize,
}

/// What to run in a terminal.
#[derive(Debug)]
pub enum Program {
    /// The environment's login shell.
    LoginShell,
    /// An explicit executable and its arguments.
    Command {
        /// Path or executable name.
        executable: OsString,
        /// Arguments passed verbatim.
        args: Vec<OsString>,
    },
}

/// The captured tail of PTY output for ACP.
#[derive(Debug)]
pub struct CapturedOutput {
    /// Original output, decoded as UTF-8.
    pub text: String,
    /// Older output was discarded.
    pub truncated: bool,
    /// Process status when the output stream has closed.
    pub exit: Option<Exit>,
}

/// An atomic attachment boundary followed by live updates.
#[derive(Debug)]
pub struct Attachment {
    /// State preceding all subsequent updates.
    pub snapshot: Snapshot,
    /// A lagged receiver needs a fresh attachment.
    pub updates: broadcast::Receiver<Arc<Update>>,
    lease: Arc<()>,
    terminal: TerminalHandle,
}

impl Attachment {
    /// Resizes the PTY and claims the lease until this viewer detaches.
    ///
    /// # Errors
    /// Returns an error if the process exited, another viewer owns the lease,
    /// or resizing fails.
    pub async fn resize(&self, size: Size, cell: CellSize) -> Result<(), PtyError> {
        self.terminal
            .request(|reply| Request::Resize(size, cell, Arc::downgrade(&self.lease), reply))
            .await
    }
}

/// A shareable terminal reference scoped to a workspace session.
#[derive(Clone, Debug)]
pub struct TerminalHandle {
    id: TerminalId,
    requests: mpsc::Sender<Request>,
    stop: watch::Sender<bool>,
    exit: watch::Receiver<Option<Exit>>,
}

impl TerminalHandle {
    /// The id returned to the caller on creation.
    #[must_use]
    pub const fn id(&self) -> TerminalId {
        self.id
    }

    /// Attaches to the current state and subscribes atomically to future
    /// updates.
    ///
    /// # Errors
    /// Returns an error if the session closed or its snapshot could not be
    /// encoded.
    pub async fn attach(&self) -> Result<Attachment, PtyError> {
        let (snapshot, updates) = self.request(Request::Attach).await?;
        Ok(Attachment {
            snapshot,
            updates,
            lease: Arc::new(()),
            terminal: self.clone(),
        })
    }

    /// Writes encoded input to the PTY.
    ///
    /// # Errors
    /// Returns an error if input is too large, the process exited, or writing
    /// fails.
    pub async fn write(&self, bytes: Vec<u8>) -> Result<(), PtyError> {
        if bytes.len() > MAX_INPUT {
            return Err(PtyError::InputTooLarge);
        }
        self.request(|reply| Request::Write(bytes, reply)).await
    }

    /// Returns the tail within the caller's UTF-8 byte limit.
    ///
    /// # Errors
    /// Returns an error if the terminal was released.
    pub async fn output(&self, limit: usize) -> Result<CapturedOutput, PtyError> {
        self.request(|reply| Request::Output(limit, reply)).await
    }

    /// Terminates the process and keeps its history.
    ///
    /// # Errors
    /// Returns an error if signaling fails or the terminal was released.
    pub async fn kill(&self) -> Result<(), PtyError> {
        self.request(Request::Kill).await
    }

    /// Waits for the process and its final output.
    ///
    /// # Errors
    /// Returns an error if the terminal is released before exit.
    pub async fn wait_for_exit(&self) -> Result<Exit, PtyError> {
        self.exit
            .clone()
            .wait_for(Option::is_some)
            .await
            .map_err(|_| PtyError::Closed)?
            .ok_or(PtyError::Closed)
    }

    async fn request<T>(&self, request: impl FnOnce(Reply<T>) -> Request) -> Result<T, PtyError> {
        let (reply, result) = oneshot::channel();
        self.requests
            .send(request(reply))
            .await
            .map_err(|_| PtyError::Closed)?;
        result.await.map_err(|_| PtyError::Closed)?
    }
}

/// The environment registry shared by ACP and interactive terminals.
#[derive(Debug, Default)]
pub struct Terminals(HashMap<TerminalId, (ThreadId, TerminalHandle)>);

impl Terminals {
    /// Starts a thread-owned terminal.
    ///
    /// # Errors
    /// Returns an error if there are no slots or the worker or process fails to
    /// start.
    pub async fn create(
        &mut self,
        thread: ThreadId,
        launch: Launch,
    ) -> Result<TerminalHandle, PtyError> {
        if self.0.len() == MAX_SESSIONS {
            return Err(PtyError::Capacity);
        }
        let id = TerminalId(uuid::Uuid::new_v4());
        let (requests, commands) = mpsc::channel(16);
        let (stop, stopping) = watch::channel(false);
        let (exited, exit) = watch::channel(None);
        let (ready, started) = oneshot::channel();
        thread::Builder::new()
            .name("viz-pty".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run(launch, commands, stopping, exited, ready)),
                    Err(error) => {
                        let _ = ready.send(Err(error.into()));
                    }
                }
            })?;
        started.await.map_err(|_| PtyError::Closed)??;
        let terminal = TerminalHandle {
            id,
            requests,
            stop,
            exit,
        };
        self.0.insert(id, (thread, terminal.clone()));
        Ok(terminal)
    }

    /// Looks up an id owned by this thread.
    #[must_use]
    pub fn get(&self, thread: ThreadId, id: TerminalId) -> Option<&TerminalHandle> {
        self.0
            .get(&id)
            .filter(|(owner, _)| *owner == thread)
            .map(|(_, terminal)| terminal)
    }

    /// Lists the terminal ids owned by this thread.
    pub fn for_thread(&self, thread: ThreadId) -> impl Iterator<Item = TerminalId> + '_ {
        self.0
            .iter()
            .filter_map(move |(id, (owner, _))| (*owner == thread).then_some(*id))
    }

    /// Stops a terminal and discards its history.
    pub fn release(&mut self, thread: ThreadId, id: TerminalId) {
        if self.get(thread, id).is_some()
            && let Some((_, terminal)) = self.0.remove(&id)
        {
            terminal.stop.send_replace(true);
        }
    }

    /// Releases all terminals belonging to the thread.
    pub fn release_thread(&mut self, thread: ThreadId) {
        self.0.retain(|_, (owner, terminal)| {
            if *owner != thread {
                return true;
            }
            terminal.stop.send_replace(true);
            false
        });
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        for (_, terminal) in self.0.values() {
            terminal.stop.send_replace(true);
        }
    }
}

type Reply<T> = oneshot::Sender<Result<T, PtyError>>;
type Attach = (Snapshot, broadcast::Receiver<Arc<Update>>);

enum Request {
    Attach(Reply<Attach>),
    Output(usize, Reply<CapturedOutput>),
    Resize(Size, CellSize, Weak<()>, Reply<()>),
    Write(Vec<u8>, Reply<()>),
    Kill(Reply<()>),
}

struct Session {
    screen: LiveScreen,
    updates: broadcast::Sender<Arc<Update>>,
    capture: VecDeque<u8>,
    truncated: bool,
    lease: Weak<()>,
    exit: Option<Exit>,
}

impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "Ghostty stays on the PTY worker thread"
    )]
    async fn flush(&mut self, pty: &mut Pty, batch: &mut Vec<u8>, live: bool) {
        if batch.is_empty() {
            return;
        }
        let excess = (self.capture.len() + batch.len()).saturating_sub(MAX_CAPTURE);
        self.truncated |= excess > 0;
        self.capture.drain(..excess);
        self.capture.extend(batch.iter().copied());
        let mut replies = Vec::new();
        let update = self.screen.output(std::mem::take(batch), |data| {
            if replies.len() + data.len() <= MAX_BATCH {
                replies.extend_from_slice(data);
            }
        });
        let _ = self.updates.send(Arc::new(update));
        if live && !replies.is_empty() {
            let _ = write(pty, &replies).await;
        }
    }

    fn capture(&self, limit: usize) -> CapturedOutput {
        let bytes = self.capture.iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bytes);
        let cut = text.ceil_char_boundary(text.len().saturating_sub(limit));
        CapturedOutput {
            text: text[cut..].to_owned(),
            truncated: self.truncated || cut > 0,
            exit: self.exit,
        }
    }

    #[expect(
        clippy::future_not_send,
        reason = "Ghostty stays on the PTY worker thread"
    )]
    async fn command(&mut self, request: Request, pty: &mut Pty, pid: Option<Pid>) {
        match request {
            Request::Attach(reply) => {
                let result = self
                    .screen
                    .snapshot()
                    .map(|state| (state, self.updates.subscribe()));
                let _ = reply.send(result.map_err(Into::into));
            }
            Request::Output(limit, reply) => {
                let _ = reply.send(Ok(self.capture(limit)));
            }
            Request::Kill(reply) => {
                let _ = reply.send(pid.map_or(Ok(()), |pid| kill(pty, pid)));
            }
            Request::Write(bytes, reply) => {
                let result = if pid.is_some() {
                    write(pty, &bytes).await.map_err(Into::into)
                } else {
                    Err(PtyError::Closed)
                };
                let _ = reply.send(result);
            }
            Request::Resize(size, cell, lease, reply) => {
                let result = if pid.is_some() {
                    self.resize(pty, size, cell, lease).await
                } else {
                    Err(PtyError::Closed)
                };
                let _ = reply.send(result);
            }
        }
    }

    #[expect(
        clippy::future_not_send,
        reason = "Ghostty stays on the PTY worker thread"
    )]
    async fn resize(
        &mut self,
        pty: &mut Pty,
        size: Size,
        cell: CellSize,
        lease: Weak<()>,
    ) -> Result<(), PtyError> {
        if lease.strong_count() == 0
            || (self.lease.strong_count() > 0 && !self.lease.ptr_eq(&lease))
        {
            return Err(PtyError::ResizeLease);
        }
        pty.resize(pty_size(size, cell))?;
        let mut replies = Vec::new();
        let update = self
            .screen
            .resize(size, cell, |data| replies.extend_from_slice(data))?;
        let _ = self.updates.send(Arc::new(update));
        self.lease = lease;
        if !replies.is_empty() {
            write(pty, &replies).await?;
        }
        Ok(())
    }
}

#[expect(
    clippy::future_not_send,
    reason = "Ghostty stays on the PTY worker thread"
)]
async fn run(
    launch: Launch,
    mut requests: mpsc::Receiver<Request>,
    mut stopping: watch::Receiver<bool>,
    exited: watch::Sender<Option<Exit>>,
    ready: Reply<()>,
) {
    let (mut pty, mut child, mut session) = match start(launch) {
        Ok(started) => started,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let pid = child
        .id()
        .and_then(|id| Pid::from_raw(i32::try_from(id).ok()?));
    let mut block = [0; READ_BYTES];
    let mut batch = Vec::new();
    let mut eof = false;
    let mut deadline = Instant::now();
    let mut closing = None;
    if ready.send(Ok(())).is_err() {
        if let Some(pid) = pid {
            let _ = kill(&pty, pid);
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
        return;
    }

    loop {
        if let Some((status, drain)) = closing
            && (eof || Instant::now() >= drain)
        {
            session.flush(&mut pty, &mut batch, false).await;
            let update = session.screen.exit(status);
            session.exit = Some(status);
            closing = None;
            let _ = session.updates.send(Arc::new(update));
            exited.send_replace(Some(status));
        }

        let live = session.exit.is_none() && closing.is_none();
        let drain = closing.map_or_else(Instant::now, |(_, drain)| drain);
        tokio::select! {
            _ = stopping.changed() => break,
            request = requests.recv() => {
                let Some(request) = request else { break };
                session.flush(&mut pty, &mut batch, live).await;
                session.command(request, &mut pty, pid.filter(|_| live)).await;
            }
            result = pty.read(&mut block[..(MAX_BATCH - batch.len()).min(READ_BYTES)]), if !eof && session.exit.is_none() => {
                if let Ok(n @ 1..) = result {
                    if batch.is_empty() {
                        deadline = Instant::now() + BATCH_DELAY;
                    }
                    batch.extend_from_slice(&block[..n]);
                    if batch.len() >= MAX_BATCH {
                        session.flush(&mut pty, &mut batch, live).await;
                    }
                } else {
                    eof = true;
                }
            }
            result = child.wait(), if live => {
                let exit = result.map_or(Exit::Lost, |status| {
                    status.code().map_or_else(|| status.signal().map_or(Exit::Lost, Exit::Signal), Exit::Code)
                });
                closing = Some((exit, Instant::now() + DRAIN_DELAY));
            }
            () = time::sleep_until(deadline), if !batch.is_empty() => {
                session.flush(&mut pty, &mut batch, live).await;
            }
            () = time::sleep_until(drain), if closing.is_some() => {}
        }
    }

    if session.exit.is_none() && closing.is_none() {
        if let Some(pid) = pid {
            let _ = kill(&pty, pid);
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

fn start(launch: Launch) -> Result<(Pty, tokio::process::Child, Session), PtyError> {
    let screen = LiveScreen::new(launch.size, launch.cell)?;
    let (pty, pts) = pty_process::open()?;
    pty.resize(pty_size(launch.size, launch.cell))?;
    let command = match launch.program {
        Program::LoginShell => {
            let shell = env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .unwrap_or_else(|| "/bin/sh".into());
            Command::new(shell).arg("-l")
        }
        Program::Command { executable, args } => Command::new(executable).args(args),
    };
    let child = command
        .current_dir(launch.cwd)
        .env_clear()
        .envs(
            RUNTIME_ENV
                .iter()
                .filter_map(|name| env::var_os(name).map(|value| (*name, value))),
        )
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .envs(launch.env)
        .kill_on_drop(true)
        .spawn(pts)?;
    let (updates, _) = broadcast::channel(16);
    Ok((
        pty,
        child,
        Session {
            screen,
            updates,
            capture: VecDeque::new(),
            truncated: false,
            lease: Weak::new(),
            exit: None,
        },
    ))
}

fn pty_size(size: Size, cell: CellSize) -> pty_process::Size {
    pty_process::Size::new_with_pixel(
        size.rows(),
        size.columns(),
        size.columns().saturating_mul(cell.width),
        size.rows().saturating_mul(cell.height),
    )
}

async fn write(pty: &mut Pty, bytes: &[u8]) -> io::Result<()> {
    time::timeout(WRITE_TIMEOUT, pty.write_all(bytes)).await?
}

/// Interactive foreground and background jobs can have separate groups from
/// the shell. The session leader is signaled last while it still owns its id.
fn kill(pty: &Pty, leader: Pid) -> Result<(), PtyError> {
    if let Ok(group) = tcgetpgrp(pty)
        && group != leader
        && getsid(Some(group)) == Ok(leader)
    {
        let _ = kill_process_group(group, Signal::KILL);
    }
    #[cfg(target_os = "linux")]
    kill_session_jobs(leader);
    match kill_process_group(leader, Signal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error).into()),
    }
}

/// Finds background job groups while the session leader still owns its id.
#[cfg(target_os = "linux")]
fn kill_session_jobs(leader: Pid) {
    use std::{collections::HashSet, fs};

    use rustix::process::getpgid;

    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    let groups: HashSet<_> = entries
        .flatten()
        .filter_map(|entry| {
            let pid = Pid::from_raw(entry.file_name().to_str()?.parse().ok()?)?;
            if getsid(Some(pid)) != Ok(leader) {
                return None;
            }
            let group = getpgid(Some(pid)).ok()?;
            (group != leader).then_some(group)
        })
        .collect();
    for group in groups {
        let _ = kill_process_group(group, Signal::KILL);
    }
}
