//! Foreground service integration; the server also runs without a service
//! manager.

use std::net::SocketAddr;

use anyhow::{Context as _, Result, ensure};
use sd_notify::NotifyState;
use tokio::{
    net::TcpListener,
    signal::unix::{SignalKind, signal},
};
use tokio_util::sync::CancellationToken;

/// Read the activation environment before creating the multithreaded runtime.
pub(crate) fn activated_listener(address: SocketAddr) -> Result<Option<std::net::TcpListener>> {
    let mut activation = listenfd::ListenFd::from_env();
    ensure!(
        activation.len() <= 1,
        "viz accepts exactly one TCP activation listener"
    );
    if activation.len() == 1 {
        let listener = activation
            .take_tcp_listener(0)
            .context("reading systemd TCP listener")?
            .context("missing activation listener")?;
        let actual = listener.local_addr()?;
        ensure!(
            actual == address,
            "activation address {actual} does not match server.listen {address}"
        );
        listener.set_nonblocking(true)?;
        return Ok(Some(listener));
    }
    Ok(None)
}

pub(crate) async fn listener(
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

pub(crate) fn ready(address: SocketAddr) -> Result<()> {
    sd_notify::notify(&[
        NotifyState::Ready,
        NotifyState::Status(&format!("Listening on {address}")),
    ])
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
