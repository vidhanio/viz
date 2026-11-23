//! Turns harness prompt errors into failures a caller can act on.

use agent_client_protocol::ErrorCode;
use serde_json::Value;
use thiserror::Error;

use crate::Error;

/// A failed ACP prompt after it was sent to the harness.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TurnFailure {
    /// The provider rejected the turn because its usage limit was reached.
    #[error("{message}")]
    UsageLimit {
        message: String,
        /// The harness's reset hint, with its original timezone and wording.
        reset: Option<String>,
    },
    /// The harness needs the user to sign in or renew their credentials.
    #[error("ACP authentication failed: {0}")]
    Authentication(String),
    /// The harness closed its transport before completing the turn.
    #[error("ACP harness exited before completing the turn: {0}")]
    HarnessCrash(String),
    /// The harness could not complete the request or sent an invalid response.
    #[error("ACP prompt failed: {0}")]
    Protocol(String),
}

pub fn classify(error: Error, prose: &str) -> Error {
    let failure = match error {
        Error::Closed(message) => TurnFailure::HarnessCrash(message),
        Error::Protocol { source, .. } => {
            let detail = source.data.as_ref().and_then(error_detail);
            let candidates = [Some(source.message.as_str()), detail, Some(prose)];
            if let Some(limit) = candidates.into_iter().flatten().find_map(usage_limit) {
                return Error::Turn(limit);
            }
            if source.code == ErrorCode::AUTH_REQUIRED {
                TurnFailure::Authentication(detail.unwrap_or(&source.message).to_owned())
            } else if let Some(message) = candidates.into_iter().flatten().find(|text| {
                let text = text.to_ascii_lowercase();
                [
                    "authentication required",
                    "authentication failed",
                    "please log in",
                    "invalid api key",
                    "unauthorized",
                ]
                .iter()
                .any(|phrase| text.contains(phrase))
            }) {
                TurnFailure::Authentication(message.trim().to_owned())
            } else {
                let message = detail.unwrap_or(&source.message);
                if message.eq_ignore_ascii_case("internal error") {
                    TurnFailure::Protocol(
                        "the harness returned an internal error without further details".to_owned(),
                    )
                } else {
                    TurnFailure::Protocol(message.to_owned())
                }
            }
        }
        other => return other,
    };
    Error::Turn(failure)
}

fn error_detail(data: &Value) -> Option<&str> {
    data.as_str()
        .or_else(|| data.get("message").and_then(Value::as_str))
        .or_else(|| data.get("details").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
}

fn usage_limit(text: &str) -> Option<TurnFailure> {
    let message = text.lines().map(str::trim).find(|line| {
        let line = line.to_ascii_lowercase();
        let Some((_, limit)) = line
            .split_once("you've hit your ")
            .or_else(|| line.split_once("you’ve hit your "))
        else {
            return false;
        };
        limit.starts_with("usage limit")
            || limit.split_once("-hour limit").is_some_and(|(hours, _)| {
                !hours.is_empty() && hours.bytes().all(|byte| byte.is_ascii_digit())
            })
    })?;
    let lower = text.to_ascii_lowercase();
    let reset = ["try again at ", "resets "]
        .iter()
        .find_map(|hint| lower.find(hint))
        .and_then(|index| text[index..].lines().next())
        .map(str::trim)
        .map(str::to_owned);
    let mut message = message
        .strip_prefix("Internal error: ")
        .unwrap_or(message)
        .to_owned();
    if let Some(reset) = &reset
        && !message.contains(reset.as_str())
    {
        message.push(' ');
        message.push_str(reset);
    }
    Some(TurnFailure::UsageLimit { message, reset })
}
