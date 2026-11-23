//! codex-acp's automatic approval reviews.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolCall, ToolStatus, truncate};

const PREFIX: &str = "guardian_assessment:";
const TITLE_LIMIT: usize = 200;

/// The verdict of a harness's own reviewer on another call.
///
/// codex-acp reports one `think` call titled `Guardian Review` next to every
/// command it assesses. The fold gives it the reviewed action as its title,
/// and a client can fold the record into the call it reviews or hide it unless
/// it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    /// The reviewed call, when the harness names it.
    pub target: Option<String>,
    /// What was reviewed, such as the command.
    pub action: Option<String>,
    /// The harness's word for the verdict, such as `approved` or `denied`.
    pub verdict: Option<String>,
    pub risk: Option<String>,
    pub authorization: Option<String>,
    pub rationale: Option<String>,
}

impl ToolCall {
    /// Whether this is a review with nothing to tell: pending or approved.
    #[must_use]
    pub fn is_routine_review(&self) -> bool {
        self.review.is_some() && self.status != ToolStatus::Failed
    }

    /// Reads the review out of the raw event codex-acp attaches.
    pub(super) fn read_review(&mut self) {
        if !self.id.starts_with(PREFIX) {
            return;
        }
        let event = self.raw_output.as_ref().or(self.raw_input.as_ref());
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
            self.title = format!("Review: {}", truncate(action, TITLE_LIMIT));
        }
        self.review = Some(review);
    }
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
