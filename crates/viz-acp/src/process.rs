//! Spawns, scrubs, and supervises harness processes.

use std::{collections::BTreeMap, path::Path, process::Stdio, sync::LazyLock, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{Error, HarnessConfig, error::Result, local_io::kill_group};

/// How long a harness has to exit after its stdin closes, before it is killed.
pub const EXIT_GRACE: Duration = Duration::from_secs(2);

pub static PROCESSES: LazyLock<TaskTracker> = LazyLock::new(TaskTracker::new);

/// Waits for dropped harnesses to exit during runtime shutdown.
pub async fn drain_processes() {
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

pub struct Harness {
    pub child: Child,
    pub stdin: tokio::process::ChildStdin,
    pub stdout: tokio::process::ChildStdout,
    pub stderr: tokio::process::ChildStderr,
}

pub fn spawn(config: &HarnessConfig, cwd: &Path) -> Result<Harness> {
    let label = &config.command;
    let mut child = Command::new(&config.command)
        .args(&config.args)
        .current_dir(cwd)
        .env_clear()
        .envs(launch_env(config)?)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| Error::Spawn {
            command: label.clone(),
            source,
        })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::MissingPipe(label.clone()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::MissingPipe(label.clone()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::MissingPipe(label.clone()))?;
    Ok(Harness {
        child,
        stdin,
        stdout,
        stderr,
    })
}

fn launch_env(config: &HarnessConfig) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for (name, secret) in &config.env {
        let value = secret
            .resolve()
            .map_err(|_| Error::Environment(name.clone()))?;
        env.insert(name.clone(), value);
    }
    let mut inherited: BTreeMap<_, _> = runtime_env().collect();
    inherited.extend(env);
    Ok(inherited)
}

pub fn runtime_env() -> impl Iterator<Item = (String, String)> {
    RUNTIME_ENV.iter().filter_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| ((*name).to_owned(), value))
    })
}

pub async fn supervise(mut child: Child, shutdown: CancellationToken, label: String) {
    let status = tokio::select! {
        status = child.wait() => status,
        () = shutdown.cancelled() => {
            if let Ok(status) = tokio::time::timeout(EXIT_GRACE, child.wait()).await {
                status
            } else {
                kill_group(&mut child);
                child.wait().await
            }
        },
    };
    shutdown.cancel();
    tracing::debug!(reason = %match status {
        Ok(status) => format!("{label} exited with {status}"),
        Err(error) => format!("{label} failed: {error}"),
    }, "ACP process exited");
}

pub async fn log(stream: impl AsyncRead + Unpin, label: String) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            tracing::info!("ACP {label}: {line}");
        }
    }
}
