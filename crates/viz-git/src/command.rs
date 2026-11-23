//! Run Git and retain bounded, sanitized diagnostics.

use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::Path,
    process::{Command, Stdio},
    sync::LazyLock,
};

use regex::Regex;
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::Error;

const DIAGNOSTIC_LIMIT: usize = 8 * 1024;
const TRACE_LIMIT: u64 = 1024 * 1024;

/// Failure classes a caller can use to present a recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFailure {
    NonFastForward,
    Authentication,
    HostKey,
    HookRejected,
    NothingToCommit,
    MissingUpstream,
    MissingIdentity,
    BranchAlreadyExists,
    Other,
}

pub fn git(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_TRACE", "0")
        .env("GIT_TRACE_PACKET", "0")
        .env("GIT_TRACE_CURL", "0")
        .env("GIT_CURL_VERBOSE", "0")
        .env("GIT_TRACE2", "0")
        .env("GIT_TRACE2_EVENT", "0")
        .env("GIT_TRACE2_PERF", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_NAMESPACE")
        .arg("--literal-pathspecs")
        .stdin(Stdio::null());
    command
}

pub fn run(
    cwd: &Path,
    operation: &'static str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Vec<u8>, Error> {
    output(git(cwd).arg(operation).args(args), operation)
}

pub fn run_read(
    cwd: &Path,
    operation: &'static str,
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> Result<Vec<u8>, Error> {
    output(
        git(cwd)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg(operation)
            .args(args),
        operation,
    )
}

pub fn output(command: &mut Command, operation: &'static str) -> Result<Vec<u8>, Error> {
    let trace = matches!(operation, "commit" | "push" | "fetch" | "switch" | "pull")
        .then(NamedTempFile::new)
        .transpose()?;
    if let Some(trace) = &trace {
        command
            .env("GIT_TRACE2_EVENT", trace.path())
            .env("GIT_TRACE2_EVENT_NESTING", "1");
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child
        .stderr
        .take()
        .ok_or(Error::Invalid("git stderr pipe"))?;
    let (result, stderr) = std::thread::scope(|scope| {
        let reader = scope.spawn(|| bounded_diagnostic(stderr));
        let result = child.wait_with_output();
        Ok::<_, Error>((
            result,
            reader
                .join()
                .map_err(|_| Error::Invalid("git stderr reader"))?,
        ))
    })?;
    let result = result?;
    if result.status.success() {
        return Ok(result.stdout);
    }
    let stderr = stderr?;
    let hook_rejected = trace
        .as_ref()
        .is_some_and(|trace| hook_rejected(trace.as_file()).unwrap_or(false));
    let stdout = diagnostic_prefix(&result.stdout);
    let message = if stderr.trim().is_empty() {
        &stdout
    } else {
        &stderr
    };
    Err(command_error(
        operation,
        result.status,
        &format!("{stderr}\n{stdout}"),
        message,
        hook_rejected,
    ))
}

pub fn clone_failure(status: std::process::ExitStatus, message: &str) -> Error {
    command_error("clone", status, message, message, false)
}

fn command_error(
    operation: &'static str,
    status: std::process::ExitStatus,
    diagnostic: &str,
    message: &str,
    hook_rejected: bool,
) -> Error {
    let kind = classify(operation, diagnostic, hook_rejected);
    let message = sanitize(message);
    let message = if message.is_empty() && hook_rejected {
        "Git hook exited unsuccessfully".to_owned()
    } else {
        message
    };
    Error::Command {
        operation,
        status,
        kind,
        message,
    }
}

/// Drains the pipe so Git never blocks on it, keeping the first whole lines
/// that fit.
fn bounded_diagnostic(pipe: impl Read) -> io::Result<String> {
    let mut pipe = BufReader::new(pipe);
    let mut kept = Vec::new();
    pipe.by_ref()
        .take(DIAGNOSTIC_LIMIT as u64 + 1)
        .read_to_end(&mut kept)?;
    io::copy(&mut pipe, &mut io::sink())?;
    Ok(diagnostic_prefix(&kept))
}

/// Whether a hook exited unsuccessfully, which a silent hook reports nowhere
/// else. The trace goes to a private file because hooks inherit its setting
/// and may capture their own commands' stderr; it can hold command arguments,
/// so it never reaches diagnostics.
fn hook_rejected(trace: &File) -> io::Result<bool> {
    let mut hooks = HookTrace::default();
    let mut lines = BufReader::new(trace.take(TRACE_LIMIT));
    let mut line = Vec::new();
    while lines.read_until(b'\n', &mut line)? != 0 {
        hooks.record(&line);
        line.clear();
    }
    Ok(hooks.rejected)
}

fn diagnostic_prefix(bytes: &[u8]) -> String {
    let bytes = if bytes.len() > DIAGNOSTIC_LIMIT {
        let prefix = &bytes[..DIAGNOSTIC_LIMIT];
        &prefix[..prefix.iter().rposition(|byte| *byte == b'\n').unwrap_or(0)]
    } else {
        bytes
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[derive(Default)]
struct HookTrace {
    children: HashSet<u64>,
    rejected: bool,
}

impl HookTrace {
    fn record(&mut self, line: &[u8]) {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(id) = event["child_id"].as_u64() else {
            return;
        };
        match event["event"].as_str() {
            Some("child_start") if event["child_class"] == "hook" => {
                self.children.insert(id);
            }
            Some("child_exit")
                if self.children.remove(&id)
                    && event["code"].as_i64().is_some_and(|code| code != 0) =>
            {
                self.rejected = true;
            }
            _ => {}
        }
    }
}

fn classify(operation: &str, message: &str, hook_rejected: bool) -> CommandFailure {
    let message = message.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| message.contains(needle));
    if hook_rejected || has(&["hook declined", "hook rejected", "hook failed"]) {
        CommandFailure::HookRejected
    } else if has(&[
        "host key verification failed",
        "remote host identification has changed",
        "no host key is known",
    ]) {
        CommandFailure::HostKey
    } else if has(&[
        "authentication failed",
        "permission denied (",
        "terminal prompts disabled",
        "invalid username or token",
        "access denied",
        "returned error: 401",
        "returned error: 403",
    ]) {
        CommandFailure::Authentication
    } else if has(&[
        "non-fast-forward",
        "(fetch first)",
        "(stale info)",
        "not possible to fast-forward",
    ]) {
        CommandFailure::NonFastForward
    } else if operation == "pull"
        && has(&[
            "there is no tracking information",
            "no upstream configured",
            "no such ref was fetched",
            "you are not currently on a branch",
        ])
    {
        CommandFailure::MissingUpstream
    } else if operation == "branch" && has(&["already exists"]) {
        CommandFailure::BranchAlreadyExists
    } else if operation == "commit"
        && has(&[
            "identity unknown",
            "unable to auto-detect email",
            "empty ident name",
            "please tell me who you are",
        ])
    {
        CommandFailure::MissingIdentity
    } else if operation == "commit"
        && has(&[
            "nothing to commit",
            "no changes added to commit",
            "nothing added to commit",
        ])
    {
        CommandFailure::NothingToCommit
    } else {
        CommandFailure::Other
    }
}

fn sanitize(message: &str) -> String {
    static ESCAPES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1b(?:\][^\x07\x1b]*(?:\x07|\x1b\\|$)|\[[0-?]*[ -/]*[@-~])")
            .expect("escape sequence regex is valid")
    });
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)[a-z][a-z0-9+.-]*://[^\s<>"'\\]+"#).expect("URL regex is valid")
    });
    let plain: String = ESCAPES
        .replace_all(message, "")
        .chars()
        .filter(|character| *character == '\n' || !character.is_control())
        .collect();
    let redacted = URL.replace_all(&plain, |captures: &regex::Captures<'_>| {
        let url = &captures[0];
        let (scheme, rest) = url.split_once("://").unwrap_or_default();
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(authority_end);
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let path = path.split(['?', '#']).next().unwrap_or_default();
        format!("{scheme}://{host}{path}")
    });
    redacted
        .trim()
        .chars()
        .scan(0, |bytes, character| {
            *bytes += character.len_utf8();
            (*bytes <= DIAGNOSTIC_LIMIT).then_some(character)
        })
        .collect()
}
