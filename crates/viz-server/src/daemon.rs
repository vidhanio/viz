//! Foreground service integration; the server also runs without a service
//! manager.

use std::{
    fs::{self, DirBuilder, Permissions},
    io::ErrorKind,
    net::SocketAddr,
    os::unix::{
        fs::{DirBuilderExt as _, FileTypeExt as _, PermissionsExt as _},
        net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream},
    },
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail, ensure};
use sd_notify::NotifyState;
use tokio::{
    net::{TcpListener, UnixListener},
    signal::unix::{SignalKind, signal},
};
use tokio_util::sync::CancellationToken;

/// Socket-activated listeners inherited from systemd before Tokio starts.
#[derive(Default)]
pub(crate) struct Activated {
    pub(crate) unix: Option<StdUnixListener>,
    pub(crate) tcp: Option<std::net::TcpListener>,
}

/// Removes a server-bound Unix socket path when the listener shuts down.
pub(crate) struct SocketGuard(Option<PathBuf>);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Read the activation environment before creating the multithreaded runtime.
pub(crate) fn activated_listeners(listen: Option<SocketAddr>) -> Result<Activated> {
    let mut activation = listenfd::ListenFd::from_env();
    let count = activation.len();
    ensure!(
        count <= 2,
        "viz accepts at most two activation listeners (Unix and TCP)"
    );
    let mut activated = Activated::default();
    for index in 0..count {
        match activation.take_unix_listener(index) {
            Ok(Some(listener)) => {
                ensure!(
                    activated.unix.is_none(),
                    "viz accepts at most one Unix activation listener"
                );
                listener.set_nonblocking(true)?;
                activated.unix = Some(listener);
            }
            Ok(None) => {}
            Err(unix_err) => match activation.take_tcp_listener(index) {
                Ok(Some(listener)) => {
                    ensure!(
                        activated.tcp.is_none(),
                        "viz accepts at most one TCP activation listener"
                    );
                    let expected =
                        listen.context("received a systemd TCP listener without server.listen")?;
                    let actual = listener.local_addr()?;
                    ensure!(
                        actual == expected,
                        "activation address {actual} does not match server.listen {expected}"
                    );
                    listener.set_nonblocking(true)?;
                    activated.tcp = Some(listener);
                }
                Ok(None) => {}
                Err(tcp_err) => {
                    bail!(
                        "activation fd {index} is neither a Unix listener ({unix_err}) nor a TCP listener ({tcp_err})"
                    );
                }
            },
        }
    }
    Ok(activated)
}

/// Adopt an activated Unix listener or bind a fresh `0600` socket inside a
/// `0700` directory, unlinking an existing socket only after a connect probe
/// confirms it is stale.
pub(crate) fn unix_listener(
    path: &Path,
    activated: Option<StdUnixListener>,
) -> Result<(UnixListener, SocketGuard)> {
    if let Some(listener) = activated {
        let listener =
            UnixListener::from_std(listener).context("adopting systemd Unix listener")?;
        return Ok((listener, SocketGuard(None)));
    }
    viz_config::validate_socket_path(path)?;
    let parent = path
        .parent()
        .context("Unix socket path has no parent directory")?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .with_context(|| format!("creating socket directory {}", parent.display()))?;
    let parent_meta = fs::symlink_metadata(parent)
        .with_context(|| format!("inspecting socket directory {}", parent.display()))?;
    ensure!(
        parent_meta.file_type().is_dir(),
        "socket directory {} must be a real directory, not a symlink",
        parent.display()
    );
    fs::set_permissions(parent, Permissions::from_mode(0o700))
        .with_context(|| format!("securing socket directory {}", parent.display()))?;
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.file_type().is_socket(),
                "refusing to replace non-socket path {}",
                path.display()
            );
            match StdUnixStream::connect(path) {
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::ConnectionRefused | ErrorKind::NotFound
                    ) =>
                {
                    fs::remove_file(path).with_context(|| {
                        format!("removing stale Unix socket {}", path.display())
                    })?;
                }
                Ok(_) => bail!("Unix socket {} is already active", path.display()),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("probing existing Unix socket {}", path.display())
                    });
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspecting Unix socket {}", path.display()));
        }
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding Unix socket {}", path.display()))?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
        .with_context(|| format!("setting permissions on Unix socket {}", path.display()))?;
    Ok((listener, SocketGuard(Some(path.to_owned()))))
}

pub(crate) async fn tcp_listener(
    address: SocketAddr,
    activated: Option<std::net::TcpListener>,
) -> Result<TcpListener> {
    match activated {
        Some(listener) => TcpListener::from_std(listener).context("adopting systemd TCP listener"),
        None => TcpListener::bind(address)
            .await
            .with_context(|| format!("listening on {address}")),
    }
}

pub(crate) fn ready(status: &str) -> Result<()> {
    sd_notify::notify(&[NotifyState::Ready, NotifyState::Status(status)])
        .context("notifying the service manager of readiness")
}

pub(crate) fn shutdown_signals(stop: CancellationToken) -> Result<tokio::task::JoinHandle<()>> {
    let mut terminate = signal(SignalKind::terminate()).context("installing SIGTERM listener")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("installing SIGINT listener")?;
    Ok(tokio::spawn(async move {
        tokio::select! {
            _ = interrupt.recv() => {},
            _ = terminate.recv() => {},
        }
        if let Err(error) = sd_notify::notify(&[
            NotifyState::Stopping,
            NotifyState::Status("Stopping ACP threads"),
        ]) {
            tracing::warn!(%error, "notifying the service manager of shutdown");
        }
        stop.cancel();
    }))
}
