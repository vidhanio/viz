//! Clone to an isolated staging directory and publish it on success.

use std::{io, path::Path, process::Stdio};

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_util::sync::CancellationToken;

use crate::{
    Error, check,
    command::{clone_failure, git},
};

/// The Git transfer stage and reported percentage, when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneProgress {
    pub phase: ClonePhase,
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClonePhase {
    Counting,
    Compressing,
    Receiving,
    Resolving,
    CheckingOut,
}

/// Clone into a fresh directory. Cancellation kills the Git child and drops its
/// staging directory.
///
/// # Errors
/// Returns an error if the destination already exists, cloning fails, or
/// cancellation wins before publication.
pub async fn clone_repository(
    url: &str,
    destination: &Path,
    cancel: &CancellationToken,
    report: impl Fn(CloneProgress) + Send + Sync,
) -> Result<(), Error> {
    let parent = destination
        .parent()
        .filter(|_| destination.is_absolute())
        .ok_or(Error::Invalid("absolute clone destination"))?;
    check(cancel)?;
    let temp = tempfile::tempdir_in(parent)?;
    let checkout = temp.path().join("checkout");
    let mut command = tokio::process::Command::from(git(parent));
    let mut child = command
        .arg("clone")
        .arg("--progress")
        .arg("--")
        .arg(url)
        .arg(&checkout)
        .env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child
        .stderr
        .take()
        .ok_or(Error::Invalid("clone stderr pipe"))?;
    let mut stderr = BufReader::new(stderr);
    let mut diagnostic = String::new();
    let read = async {
        let mut fragment = Vec::new();
        loop {
            let buffer = stderr.fill_buf().await?;
            if buffer.is_empty() {
                break;
            }
            let break_at = buffer.iter().position(|byte| matches!(byte, b'\r' | b'\n'));
            let length = break_at.map_or(buffer.len(), |at| at + 1);
            if fragment.len() + length <= 8192 {
                fragment.extend_from_slice(&buffer[..length]);
            } else if fragment.len() <= 8192 {
                fragment.resize(8193, b' ');
            }
            stderr.consume(length);
            if break_at.is_some() {
                if fragment.len() <= 8192 {
                    let line = String::from_utf8_lossy(&fragment);
                    if let Some(progress) = progress(line.trim()) {
                        report(progress);
                    } else if diagnostic.len() + line.len() <= 8192 {
                        diagnostic.push_str(&line);
                    }
                }
                fragment.clear();
            }
        }
        Ok::<_, io::Error>(())
    };
    tokio::select! {
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(Error::Cancelled);
        }
        result = read => result?,
    }
    let status = tokio::select! {
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(Error::Cancelled);
        }
        status = child.wait() => status?,
    };
    if !status.success() {
        return Err(clone_failure(status, &diagnostic));
    }
    check(cancel)?;
    tokio::fs::create_dir(destination).await?;
    if let Err(error) = tokio::fs::rename(&checkout, destination).await {
        let _ = tokio::fs::remove_dir(destination).await;
        return Err(error.into());
    }
    Ok(())
}

fn progress(line: &str) -> Option<CloneProgress> {
    let (label, value) = line.trim_start_matches("remote: ").split_once(':')?;
    let phase = match label {
        "Counting objects" | "Enumerating objects" => ClonePhase::Counting,
        "Compressing objects" => ClonePhase::Compressing,
        "Receiving objects" => ClonePhase::Receiving,
        "Resolving deltas" => ClonePhase::Resolving,
        "Updating files" => ClonePhase::CheckingOut,
        _ => return None,
    };
    let percent = value
        .split_once('%')
        .and_then(|(number, _)| number.trim().parse::<u8>().ok())
        .filter(|value| *value <= 100);
    Some(CloneProgress { phase, percent })
}
