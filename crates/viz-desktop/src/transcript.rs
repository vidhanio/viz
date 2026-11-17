//! Turns a pi-durable `ConversationView` into the lines the transcript shows.

use serde_json::Value;

/// Longest tool-call summary, in characters, before it's cut with an ellipsis.
const SUMMARY_CHARS: usize = 96;

/// Who a transcript line is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The user's input.
    User,
    /// The assistant's text.
    Assistant,
    /// An assistant tool call, summarized on one line.
    Tool,
}

/// One displayed line (a paragraph for text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Stable element key: entry id, message index, part index.
    pub key: String,
    /// Who it's from.
    pub role: Role,
    /// What it says.
    pub text: String,
}

/// The view's user and assistant text, and its tool calls as one-line
/// summaries, in entry order.
pub fn lines(view: &Value) -> Vec<Line> {
    let mut lines = Vec::new();
    let entries = view
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for entry in entries {
        let id = entry
            .get("id")
            .map_or_else(String::new, ToString::to_string);
        let Some(messages) = entry.get("model").and_then(Value::as_array) else {
            continue;
        };
        for (m, message) in messages.iter().enumerate() {
            let role = match message.get("role").and_then(Value::as_str) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => continue,
            };
            let mut push = |p: usize, role, text: String| {
                if !text.trim().is_empty() {
                    lines.push(Line {
                        key: format!("{id}-{m}-{p}"),
                        role,
                        text,
                    });
                }
            };
            match message.get("content") {
                Some(Value::String(text)) => push(0, role, text.clone()),
                Some(Value::Array(parts)) => {
                    for (p, part) in parts.iter().enumerate() {
                        match part.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let text =
                                    part.get("text").and_then(Value::as_str).unwrap_or_default();
                                push(p, role, text.to_owned());
                            }
                            Some("toolCall") if role == Role::Assistant => {
                                push(p, Role::Tool, tool_summary(part));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    lines
}

/// `name {"arg":…}`, cut to [`SUMMARY_CHARS`].
fn tool_summary(call: &Value) -> String {
    let name = call.get("name").and_then(Value::as_str).unwrap_or("tool");
    let arguments = call
        .get("arguments")
        .map_or_else(String::new, ToString::to_string);
    let summary = format!("{name} {arguments}");
    match summary.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", &summary[..cut]),
        None => summary,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shows_text_and_summarizes_tool_calls() {
        let view = json!({
            "conversation": { "id": 1 },
            "docs": {},
            "entries": [
                { "id": 1, "kind": "pi.user", "model": [{ "role": "user", "content": "hi" }] },
                { "id": 2, "kind": "viz.input", "model": [{ "role": "user", "content": [{ "type": "text", "text": "yo" }] }] },
                { "id": 3, "kind": "pi.assistant", "model": [{ "role": "assistant", "content": [
                    { "type": "thinking", "thinking": "hmm" },
                    { "type": "text", "text": "" },
                    { "type": "toolCall", "id": "t", "name": "read", "arguments": { "path": "a" } },
                    { "type": "text", "text": "done" },
                ] }] },
                { "id": 4, "kind": "pi.toolResult", "model": [{ "role": "toolResult", "content": [{ "type": "text", "text": "x" }] }] },
                { "id": 5, "kind": "bookkeeping" },
            ],
        });
        let line = |key: &str, role, text: &str| Line {
            key: key.to_owned(),
            role,
            text: text.to_owned(),
        };
        assert_eq!(
            lines(&view),
            [
                line("1-0-0", Role::User, "hi"),
                line("2-0-0", Role::User, "yo"),
                line("3-0-2", Role::Tool, r#"read {"path":"a"}"#),
                line("3-0-3", Role::Assistant, "done"),
            ]
        );
    }

    #[test]
    fn cuts_long_summaries() {
        let summary =
            tool_summary(&json!({ "name": "bash", "arguments": { "command": "x".repeat(200) } }));
        assert_eq!(summary.chars().count(), SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
    }
}
