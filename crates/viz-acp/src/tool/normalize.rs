//! Normalizes runner-specific tool arguments and results at ingestion.

use std::borrow::Cow;

use serde_json::Value;
use viz_protocol::{
    ContentKind, Exit, FileOperation, FileTarget, Location, SearchMatch, ToolCall, ToolCallStatus,
    ToolContent, ToolPayload,
};

const MAX_SEARCH_MATCHES: usize = 60;
const SEARCH_OUTPUT_BYTES: usize = 32_768;
const SNIPPET_CHARS: usize = 512;
const ARGUMENT_CHARS: usize = 4_000;
const INPUT_CHARS: usize = 4_000;

const OUTPUT_FIELDS: [&str; 6] = [
    "formatted_output",
    "aggregated_output",
    "output",
    "content",
    "text",
    "result",
];

#[expect(
    clippy::too_many_lines,
    reason = "maps each tool category in one ACP argument dispatch"
)]
pub(super) fn normalize_input(call: &mut ToolCall) {
    let plain = call.raw_input.as_ref().and_then(Value::as_str);
    let parsed = plain.and_then(|text| serde_json::from_str::<Value>(text).ok());
    let args = parsed
        .as_ref()
        .or(call.raw_input.as_ref())
        .filter(|value| value.is_object());
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| args?.get(*key)?.as_str())
            .filter(|text| !text.trim().is_empty())
    };
    let number = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| args?.get(*key)?.as_u64())
            .filter(|n| *n > 0)
    };
    let kind = call.kind.as_deref().filter(|kind| *kind != "other");
    let name = call.name.as_deref().unwrap_or_default();
    let category = match name {
        "bash" | "read" | "read_file" | "write" | "write_file" | "edit" | "edit_file"
        | "apply_patch" | "grep" | "glob" | "web_search" | "web_fetch" => name,
        _ => kind.unwrap_or(name),
    };
    let paths = || {
        let path = field(&[
            "path",
            "file_path",
            "filePath",
            "absolute_path",
            "FilePath",
            "AbsolutePath",
            "TargetFile",
        ]);
        let line = |keys| number(keys).and_then(|n| u32::try_from(n).ok());
        let start = line(&["StartLine", "start_line", "startLine", "line"]);
        let offset = line(&["offset", "Offset"]);
        let end = line(&["EndLine", "end_line", "endLine"]).or_else(|| {
            if start.is_some() {
                return None;
            }
            offset.map(|line| line.saturating_add(line_limit(args)))
        });
        let mut targets = call.locations.clone();
        if let Some(path) = path {
            let target = file_target(path, start.or(offset), end);
            if let Some(existing) = targets
                .iter_mut()
                .find(|item| item.path.ends_with(&target.path))
            {
                let line = existing.location.line.or(target.location.line);
                let end = target.location.end_line.or(existing.location.end_line);
                existing.location = location(line, end);
            } else if targets.is_empty() {
                targets.push(target);
            }
        } else if let [target] = targets.as_mut_slice() {
            target.location = location(
                target.location.line.or(start).or(offset),
                end.or(target.location.end_line),
            );
        }
        for diff in call.diffs() {
            let target = file_target(&diff.path, None, None);
            if targets.iter().all(|item| item.path != target.path) {
                targets.push(target);
            }
        }
        targets
    };
    let edit = |operation| ToolPayload::Edit {
        operation,
        targets: paths(),
    };
    let summary = || {
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
        .filter(|text| !text.starts_with("xd://mcp__viz_"))
        .or_else(|| parsed.as_ref().and_then(Value::as_str))
        .map(|text| cap(text, INPUT_CHARS))
    };
    call.payload = match category {
        "bash" | "execute" | "shell" | "terminal" => ToolPayload::Execute {
            command: field(&[
                "command",
                "cmd",
                "CommandLine",
                "commandLine",
                "command_line",
            ])
            .or_else(|| parsed.as_ref().and_then(Value::as_str))
            .or_else(|| {
                plain.filter(|text| args.is_none() && !text.trim_start().starts_with(['{', '[']))
            })
            .or_else(|| call.title.strip_prefix("$ "))
            .map(|text| cap(text, INPUT_CHARS)),
            timeout_ms: number(&["timeout_ms", "timeoutMs"])
                .or_else(|| number(&["timeout"]).and_then(|n| n.checked_mul(1_000))),
        },
        "read" | "read_file" => ToolPayload::Read { targets: paths() },
        "write" | "create" | "write_file" => edit(FileOperation::Write),
        "edit" | "edit_file" | "apply_patch" => edit(FileOperation::Edit),
        "delete" => edit(FileOperation::Delete),
        "move" => edit(FileOperation::Move),
        "search" | "grep" | "glob" | "web_search" => ToolPayload::Search {
            query: field(&[
                "query",
                "pattern",
                "path",
                "Query",
                "SearchQuery",
                "Pattern",
            ])
            .or_else(|| plain.filter(|_| args.is_none()))
            .map(|text| cap(text, INPUT_CHARS)),
            matches: Vec::new(),
        },
        "fetch" | "web_fetch" => ToolPayload::Fetch {
            url: field(&["url", "URL", "Url"])
                .or_else(|| plain.filter(|_| args.is_none()))
                .map(|text| cap(text, INPUT_CHARS)),
            matches: Vec::new(),
        },
        _ if call.diffs().next().is_some() => edit(FileOperation::Edit),
        _ if name.starts_with("mcp__") => ToolPayload::Mcp { summary: summary() },
        _ => ToolPayload::Other { summary: summary() },
    };
    call.arguments = args
        .and_then(|value| serde_json::to_string_pretty(value).ok())
        .map(|text| cap(&text, ARGUMENT_CHARS));
}

fn line_limit(args: Option<&Value>) -> u32 {
    ["limit", "Limit"]
        .iter()
        .find_map(|key| args?.get(*key)?.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1)
        .saturating_sub(1)
}

pub(super) fn location(line: Option<u32>, end_line: Option<u32>) -> Location {
    let line = line.filter(|line| *line > 0);
    Location {
        line,
        end_line: line.map(|start| end_line.unwrap_or(start).max(start)),
    }
}

fn file_target(path: &str, line: Option<u32>, end_line: Option<u32>) -> FileTarget {
    FileTarget {
        path: path.into(),
        location: location(line, end_line),
    }
}

fn positive_line(text: &str) -> Option<u32> {
    text.parse().ok().filter(|n| *n > 0)
}

fn cap(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

pub fn normalize_output(call: &mut ToolCall) {
    let command = matches!(call.payload, ToolPayload::Execute { .. });
    let search = matches!(
        call.payload,
        ToolPayload::Search { .. } | ToolPayload::Fetch { .. }
    );
    let preserve = command
        || matches!(
            call.payload,
            ToolPayload::Read { .. } | ToolPayload::Search { .. }
        );
    let delegation = call
        .viz
        .as_ref()
        .is_some_and(|link| link.tool == "delegate_task");
    let head = call
        .content
        .iter()
        .find_map(|item| match item {
            ToolContent::Block(block) if block.kind == ContentKind::Text => block
                .payload
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty()),
            _ => None,
        })
        .or_else(|| {
            call.terminals
                .iter()
                .map(|terminal| terminal.output.as_str())
                .find(|text| !text.trim().is_empty())
        });
    let head = head.map(|text| call.trim_output(text));
    if let Some(head) = head
        && !head.is_empty()
        && !search
        && !(command && head.starts_with("Created At: "))
        && !delegation
        && (preserve || !head.starts_with(['{', '[']))
    {
        call.display_output = None;
        call.task_id = None;
        return;
    }
    let structured = call.structured_output_text();
    let fallback = structured.is_none();
    let source = structured
        .or_else(|| {
            call.raw_output.as_ref().map(|raw| {
                raw_text(raw, 0)
                    .unwrap_or_else(|| serde_json::to_string_pretty(raw).unwrap_or_default())
            })
        })
        .unwrap_or_default();
    let source = call.trim_output(&source);
    let envelope = if command || delegation {
        antigravity_output(source, call.status)
    } else {
        None
    };
    let (body, exit) = envelope.unwrap_or((source, None));
    let (body, task_id) = readable_output(body, preserve, delegation);
    let hits = match call.payload {
        ToolPayload::Search { .. } => {
            parse_search_matches(&body, call.name.as_deref() == Some("web_search"))
        }
        ToolPayload::Fetch { .. } => parse_search_matches(&body, true),
        _ => Vec::new(),
    };
    let display = (fallback || body != source).then(|| body.into_owned());
    if call.exit.is_none()
        && let Some(code) = exit
    {
        call.exit = Some(Exit {
            code: Some(i64::from(code)),
            signal: None,
        });
    }
    call.task_id = task_id;
    match &mut call.payload {
        ToolPayload::Search { matches, .. } | ToolPayload::Fetch { matches, .. } => *matches = hits,
        _ => {}
    }
    call.display_output = display;
}

/// Extracts structured `path:line:snippet` or URL lines from search tool
/// output.
fn parse_search_matches(output: &str, web: bool) -> Vec<SearchMatch> {
    let mut matches = Vec::new();
    let output = if output.len() > SEARCH_OUTPUT_BYTES {
        output[..output.floor_char_boundary(SEARCH_OUTPUT_BYTES)]
            .rsplit_once('\n')
            .map_or("", |(lines, _)| lines)
    } else {
        output
    };
    for raw in output.lines() {
        if matches.len() >= MAX_SEARCH_MATCHES {
            break;
        }
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if (web || line.starts_with("http://") || line.starts_with("https://"))
            && let Some(url_start) = line.find("https://").or_else(|| line.find("http://"))
        {
            let prefix = line[..url_start]
                .trim()
                .trim_end_matches([':', '-', '—', '|']);
            let rest = &line[url_start..];
            let (url, trailing) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(u, r)| (u, r.trim()));
            let snippet = if prefix.is_empty() {
                cap(trailing, SNIPPET_CHARS)
            } else {
                cap(prefix, SNIPPET_CHARS)
            };
            matches.push(SearchMatch::Web {
                url: url.trim_end_matches([')', ']', ',', '.']).to_owned(),
                snippet,
            });
            continue;
        }
        if let Some((path, rest)) = line.split_once(':')
            && !path.is_empty()
            && !path.contains(char::is_whitespace)
        {
            if let Some((num_str, snippet)) = rest.split_once(':')
                && let Some(num) = positive_line(num_str)
            {
                matches.push(SearchMatch::File {
                    target: file_target(path, Some(num), None),
                    snippet: cap(snippet.trim(), SNIPPET_CHARS),
                });
                continue;
            }
            if let Some(num) = positive_line(rest.trim()) {
                matches.push(SearchMatch::File {
                    target: file_target(path, Some(num), None),
                    snippet: String::new(),
                });
                continue;
            }
        }
        if !line.contains(char::is_whitespace) && (line.contains('/') || line.contains('.')) {
            matches.push(SearchMatch::File {
                target: file_target(line, None, None),
                snippet: String::new(),
            });
        }
    }
    matches
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

/// Extracts prose from known result fields. A confirmed
/// delegation can get its task ID from the same result before it is hidden.
fn readable_output(
    output: &str,
    preserve_output: bool,
    delegation: bool,
) -> (Cow<'_, str>, Option<String>) {
    if (preserve_output && !delegation) || !output.trim_start().starts_with(['{', '[']) {
        return (Cow::Borrowed(output), None);
    }
    let parsed = serde_json::from_str::<Value>(output);
    let task_id = if delegation {
        let id = parsed
            .as_ref()
            .ok()
            .and_then(|value| value.get("task")?.get("id"));
        id.and_then(Value::as_str).map(str::to_owned)
    } else {
        None
    };
    let text = if preserve_output {
        Cow::Borrowed(output)
    } else {
        match parsed {
            Ok(value) => json_text(value).map_or(Cow::Borrowed(output), Cow::Owned),
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

/// Derives the current tool shape from a historical stored record before
/// projection.
pub fn normalize_stored_tool(call: &mut ToolCall) {
    for target in &mut call.locations {
        target.location = location(target.location.line, target.location.end_line);
    }
    normalize_input(call);
    normalize_output(call);
}

/// Readable text from ACP `rawOutput`, whose shape is runner-specific: a
/// string, text content blocks, a known text field, or `stdout`/`stderr`;
/// `None` for anything else.
fn raw_text(value: &Value, depth: u8) -> Option<String> {
    match value {
        Value::Null => Some(String::new()),
        Value::String(text) => Some(text.clone()),
        _ if depth > 2 => None,
        Value::Array(items) => items
            .iter()
            .map(|item| raw_text(item, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(|parts| parts.join("\n")),
        Value::Object(record) => {
            let field = OUTPUT_FIELDS.iter().find_map(|field| {
                record
                    .get(*field)
                    .and_then(|value| raw_text(value, depth + 1))
                    .filter(|text| !text.is_empty())
            });
            field.or_else(|| {
                let streams: Vec<&str> = ["stdout", "stderr"]
                    .iter()
                    .filter_map(|name| record.get(*name).and_then(Value::as_str))
                    .filter(|text| !text.is_empty())
                    .collect();
                (!streams.is_empty()).then(|| streams.join("\n"))
            })
        }
        Value::Bool(_) | Value::Number(_) => None,
    }
}
