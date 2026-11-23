//! Pure projection from wire [`ToolCall`] records into renderable [`Tool`]
//! rows: command extraction, Antigravity terminal-envelope stripping, readable
//! JSON unwrapping, and output length capping.

use std::borrow::Cow;

use serde_json::Value;
use viz_client::protocol::{ToolCall, ToolCallStatus};

use super::transcript::{Tool, ToolStatus};

/// Longest tool-call summary, in characters.
const SUMMARY_CHARS: usize = 96;
/// Longest command text shown in a tooltip, in characters.
const COMMAND_CHARS: usize = 4_000;
/// Longest tool output an expanded row shows, in characters (its tail for
/// running output, its head for a result).
const OUTPUT_CHARS: usize = 6_000;

/// Projects an ACP [`ToolCall`] into a renderable [`Tool`] row.
#[must_use]
pub(super) fn parse_tool(turn: Option<&str>, record: &ToolCall) -> Tool {
    let name = match record.name.as_deref() {
        Some(
            "bash" | "read" | "read_file" | "write" | "write_file" | "edit" | "edit_file"
            | "apply_patch" | "grep" | "glob",
        ) => record.name.as_deref(),
        _ => record
            .kind
            .as_deref()
            .filter(|kind| *kind != "other")
            .or(record.name.as_deref()),
    }
    .unwrap_or("tool");
    let command = is_command(name);
    let plain_input = record
        .raw_input
        .as_ref()
        .and_then(serde_json::Value::as_str);
    let parsed = plain_input.and_then(|input| serde_json::from_str(input).ok());
    let arguments = parsed
        .as_ref()
        .or_else(|| record.raw_input.as_ref().filter(|value| value.is_object()));
    let (summary, mut input, timeout_ms) = tool_input(name, arguments);
    if input.is_none() && command {
        input = plain_input
            .filter(|text| arguments.is_none() && !text.trim_start().starts_with(['{', '[']))
            .or_else(|| record.title.strip_prefix("$ "))
            .map(|text| cap_head(text, COMMAND_CHARS));
    }
    let rendered = record.output_text();
    let output = rendered.as_deref().unwrap_or_default();
    let delegation = record
        .viz
        .as_ref()
        .is_some_and(|viz| viz.tool == "delegate_task");
    let (output, legacy_exit) = if command || delegation {
        antigravity_output(output, record.status).unwrap_or((output, None))
    } else {
        (output, None)
    };
    let exit_code = record
        .exit
        .as_ref()
        .and_then(|exit| exit.code)
        .and_then(|code| i32::try_from(code).ok())
        .or(legacy_exit);
    let preserve_output = command || matches!(name, "read" | "read_file");
    let (output, task_id) = readable_output(output, preserve_output, delegation);
    let status = match record.status {
        ToolCallStatus::Pending => ToolStatus::Pending,
        ToolCallStatus::InProgress => ToolStatus::Running,
        ToolCallStatus::Completed if exit_code.is_some_and(|code| code != 0) => ToolStatus::Failed,
        ToolCallStatus::Completed => ToolStatus::Done,
        ToolCallStatus::Failed => ToolStatus::Failed,
    };
    let summary = if summary.is_empty() {
        let title = if record.title == record.id {
            ""
        } else {
            &record.title
        };
        first_line(
            input
                .as_deref()
                .or_else(|| (!title.is_empty()).then_some(title))
                .or_else(|| {
                    record
                        .locations
                        .first()
                        .and_then(|location| location.path.to_str())
                })
                .unwrap_or_default(),
            SUMMARY_CHARS,
        )
    } else {
        summary
    };
    Tool {
        key: format!("acp-{}-{}", turn.unwrap_or_default(), record.id),
        call_id: record.id.clone(),
        entry: None,
        name: name.to_owned(),
        summary,
        input,
        pending_input: (status == ToolStatus::Running)
            .then(|| record.raw_input.as_ref().map(serde_json::Value::to_string))
            .flatten(),
        timeout_ms,
        started_at: record.started_at,
        status,
        exit_code,
        output: cap_output(output, status),
        task_id,
        child: record.viz.as_ref().and_then(|viz| viz.child),
        child_environment: record.viz.as_ref().and_then(|viz| viz.environment.clone()),
        viz: record.viz.as_ref().map(|viz| viz.tool.clone()),
        diffs: record.diffs().cloned().collect(),
    }
}

/// Whether `name` identifies a shell/command-execution tool.
#[must_use]
pub fn is_command(name: &str) -> bool {
    matches!(name, "bash" | "execute" | "shell" | "terminal")
}

/// Antigravity sends the model-facing terminal envelope as ACP content:
/// <https://github.com/google-antigravity/antigravity-cli/issues/1506>
fn antigravity_output(output: &str, status: ToolCallStatus) -> Option<(&str, Option<i32>)> {
    let (timestamp, rest) = output_line(output.strip_prefix("Created At: ")?);
    if !terminal_timestamp(timestamp) {
        return None;
    }
    let (line, tail) = output_line(rest);
    let rest = if line
        .strip_prefix("Completed At: ")
        .is_some_and(terminal_timestamp)
    {
        tail
    } else {
        rest
    };
    let rest = rest.trim_start_matches(['\n', '\r']);
    let (line, tail) = output_line(rest);
    let exit_code = line
        .strip_prefix("Command exited with code ")
        .and_then(|code| code.strip_suffix('.'))
        .and_then(|code| code.parse().ok());
    let rest = if exit_code.is_some() {
        tail.trim_start_matches(['\n', '\r'])
    } else {
        rest
    };
    let (heading, body) = output_line(rest);
    if heading == "Command output:" {
        return Some((body, exit_code));
    }
    let unfinished = matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress);
    (rest.is_empty() && (exit_code.is_some() || unfinished)).then_some(("", exit_code))
}

fn output_line(text: &str) -> (&str, &str) {
    let (line, rest) = text.split_once('\n').unwrap_or((text, ""));
    (line.strip_suffix('\r').unwrap_or(line), rest)
}

fn terminal_timestamp(timestamp: &str) -> bool {
    let Some((date_time, zone)) = timestamp.split_at_checked(19) else {
        return false;
    };
    let matches_format = |text: &str, shape: &[u8]| {
        text.len() == shape.len()
            && text
                .bytes()
                .zip(shape)
                .all(|(byte, &expected)| match expected {
                    b'#' => byte.is_ascii_digit(),
                    _ => byte == expected,
                })
    };
    matches_format(date_time, b"####-##-##T##:##:##")
        && (zone == "Z"
            || zone
                .strip_prefix(['+', '-'])
                .is_some_and(|offset| matches_format(offset, b"##:##")))
}

/// Keep prose and known result fields, not opaque RPC objects. A confirmed
/// delegation can get its task ID from the same result before it is hidden.
fn readable_output(
    output: &str,
    preserve_output: bool,
    delegation: bool,
) -> (Cow<'_, str>, Option<String>) {
    if (preserve_output && !delegation) || !output.trim_start().starts_with(['{', '[']) {
        return (Cow::Borrowed(output), None);
    }
    let mut parsed = serde_json::from_str::<Value>(output);
    let task_id = if delegation {
        let id = parsed
            .as_mut()
            .ok()
            .and_then(|value| value.get_mut("task")?.get_mut("id"));
        match id.map(Value::take) {
            Some(Value::String(id)) => Some(id),
            _ => None,
        }
    } else {
        None
    };
    let text = if preserve_output {
        Cow::Borrowed(output)
    } else {
        match parsed {
            Ok(value) => Cow::Owned(json_text(value).unwrap_or_default()),
            Err(error) if error.is_eof() => Cow::Borrowed(""),
            Err(_) => Cow::Borrowed(output),
        }
    };
    (text, task_id)
}

fn json_text(value: Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.trim().is_empty()).then_some(text),
        Value::Object(mut fields) => ["text", "message", "error", "output", "content"]
            .into_iter()
            .find_map(|key| fields.remove(key).and_then(json_text)),
        Value::Array(items) => items
            .into_iter()
            .filter_map(json_text)
            .reduce(|mut text, next| {
                text.push('\n');
                text.push_str(&next);
                text
            }),
        _ => None,
    }
}

/// Project tool-specific fields out of ACP arguments. Unknown structure stays
/// in the transcript record, not in the presentation.
fn tool_input(name: &str, arguments: Option<&Value>) -> (String, Option<String>, Option<u64>) {
    let Some(args) = arguments else {
        return (String::new(), None, None);
    };
    let field = |keys: &[&str]| keys.iter().find_map(|key| str_at(args, key));
    let command = if is_command(name) {
        field(&[
            "command",
            "cmd",
            "CommandLine",
            "commandLine",
            "command_line",
        ])
        .or_else(|| args.as_str())
    } else {
        None
    };
    let summary = command
        .or_else(|| {
            field(&[
                "path",
                "file_path",
                "filePath",
                "absolute_path",
                "task",
                "query",
                "pattern",
                "url",
                "prompt",
                "i",
            ])
        })
        .filter(|text| !text.starts_with("xd://mcp__viz_"))
        .or_else(|| args.as_str())
        .map_or_else(String::new, |text| first_line(text, SUMMARY_CHARS));
    let input = command.map(|text| cap_head(text, COMMAND_CHARS));
    let millis = |key| args.get(key).and_then(Value::as_u64);
    let timeout = millis("timeout_ms")
        .or_else(|| millis("timeoutMs"))
        .or_else(|| millis("timeout").and_then(|n| n.checked_mul(1_000)))
        .filter(|ms| *ms > 0);
    (summary, input, timeout)
}

/// The first non-empty line of `text`, cut to `max` characters, with an
/// ellipsis when anything was left out.
fn first_line(text: &str, max: usize) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let cut = cap_head(lines.next().unwrap_or_default(), max);
    if lines.next().is_some() && !cut.ends_with('…') {
        format!("{cut} …")
    } else {
        cut
    }
}

/// `text`'s first `max` characters, with an ellipsis if it was longer.
fn cap_head(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

fn cap_output(text: Cow<'_, str>, status: ToolStatus) -> String {
    let live = matches!(status, ToolStatus::Running | ToolStatus::Pending);
    let cut = if live {
        text.char_indices()
            .rev()
            .nth(OUTPUT_CHARS)
            .map(|(cut, ch)| cut + ch.len_utf8())
    } else {
        text.char_indices().nth(OUTPUT_CHARS).map(|(cut, _)| cut)
    };
    let Some(cut) = cut else {
        return text.into_owned();
    };
    match text {
        Cow::Borrowed(text) if live => format!("…{}", &text[cut..]),
        Cow::Borrowed(text) => format!("{}…", &text[..cut]),
        Cow::Owned(mut text) => {
            let range = if live { 0..cut } else { cut..text.len() };
            text.replace_range(range, "…");
            text
        }
    }
}

fn str_at<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}
