//! codex-acp's automatic approval reviews.

use serde_json::Value;
use viz_protocol::{Review, ToolCall};

const PREFIX: &str = "guardian_assessment:";
const TITLE_LIMIT: usize = 200;

/// Reads the review out of the raw event codex-acp attaches.
pub(super) fn read(call: &mut ToolCall) {
    if !call.id.starts_with(PREFIX) {
        return;
    }
    let event = call.raw_output.as_ref().or(call.raw_input.as_ref());
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let verdict = event.and_then(|event| event.get("review"));
    let review = Review {
        target: text(event.and_then(|event| event.get("targetItemId"))),
        action: event
            .and_then(|event| event.get("action"))
            .and_then(action_text),
        verdict: text(verdict.and_then(|review| review.get("status"))),
        risk: text(verdict.and_then(|review| review.get("riskLevel"))),
        authorization: text(verdict.and_then(|review| review.get("userAuthorization"))),
        rationale: text(verdict.and_then(|review| review.get("rationale"))),
    };
    if let Some(action) = &review.action {
        call.title = format!(
            "Review: {}",
            action.chars().take(TITLE_LIMIT).collect::<String>()
        );
    }
    call.review = Some(review);
}

fn action_text(action: &Value) -> Option<String> {
    let field = |name: &str| action.get(name).and_then(Value::as_str).map(str::to_owned);
    let list = |name: &str| {
        let items: Vec<&str> = action
            .get(name)?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        (!items.is_empty()).then(|| items.join(" "))
    };
    match action.get("type").and_then(Value::as_str)? {
        "command" => field("command"),
        "execve" => list("argv").or_else(|| field("program")),
        "applyPatch" => list("files").map(|files| format!("edit {files}")),
        "networkAccess" => field("target")
            .filter(|target| !target.is_empty())
            .or_else(|| field("host"))
            .map(|target| format!("network access to {target}")),
        "mcpToolCall" => {
            field("toolName").map(
                |tool| match field("connectorName").or_else(|| field("server")) {
                    Some(server) => format!("{tool} on {server}"),
                    None => tool,
                },
            )
        }
        "writeStdin" => Some("write to a running command".to_owned()),
        "requestPermissions" => field("reason").or_else(|| Some("more permissions".to_owned())),
        other => Some(other.to_owned()),
    }
}
