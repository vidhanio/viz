//! Harness requests and access modes: which requests wait for an answer,
//! which answers are valid, and how a request stops waiting.
//!
//! A thread's access mode is either one of the harness's own modes (its `mode`
//! option, normalised to an [`AccessKind`]) or [`AccessKind::Full`] provided
//! by viz, which approves every permission request itself. With nothing
//! chosen for the thread or its project, viz approves every request, as it
//! always has. Questions always wait for an answer.

use thiserror::Error;
use viz_protocol::{
    Access, AccessKind, AccessMode, AccessSource, AnsweredBy, ConfigOption, NativeAccess,
    OptionKind, OptionValue, PermissionKind, RequestAnswer, RequestBody, RequestState, RunStatus,
    ThreadId, Withdrawal,
};

/// The option category of a harness's own mode selector.
const MODE: &str = "mode";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Rejection {
    #[error("the request was already answered or withdrawn")]
    Settled,
    #[error("the answer does not fit the request: {0}")]
    InvalidAnswer(&'static str),
    #[error("only the user or the delegating thread's agent may answer")]
    NotAllowed,
    #[error("that access mode is not offered for this thread")]
    UnknownAccess,
}

/// What happens to a request when it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Answer at once without recording anything.
    Answer(RequestAnswer),
    /// Record the request and wait for an answer.
    Wait,
}

/// The modes a thread can choose from: the harness's own, then viz's full
/// access unless the harness has a full-access mode of its own.
#[must_use]
pub fn access_modes(options: &[ConfigOption]) -> Vec<AccessMode> {
    let mut modes: Vec<AccessMode> = options
        .iter()
        .filter(|option| {
            option.category.as_deref() == Some(MODE) && option.kind == OptionKind::Select
        })
        .flat_map(|option| {
            option.choices.iter().map(|choice| AccessMode {
                kind: kind_of(&choice.value),
                label: choice.name.clone(),
                native: Some(NativeAccess {
                    option: option.id.clone(),
                    value: choice.value.clone(),
                }),
            })
        })
        .collect();
    if !modes.iter().any(|mode| mode.kind == AccessKind::Full) {
        modes.push(viz_full_access());
    }
    modes
}

fn viz_full_access() -> AccessMode {
    AccessMode {
        kind: AccessKind::Full,
        label: "Full access".to_owned(),
        native: None,
    }
}

/// Classifies a harness mode by its value, as the installed harnesses name
/// them.
fn kind_of(value: &str) -> AccessKind {
    let value: String = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match value.as_str() {
        "bypasspermissions" | "agentfullaccess" | "fullaccess" | "full" | "yolo" => {
            AccessKind::Full
        }
        "auto" | "agent" | "workspacewrite" => AccessKind::Auto,
        "acceptedits" => AccessKind::AcceptEdits,
        "plan" | "readonly" => AccessKind::ReadOnly,
        _ => AccessKind::Ask,
    }
}

/// The thread's access: its own choice, else the project's, else viz's full
/// access. A chosen harness mode follows the harness when the harness changes
/// mode itself, as when it leaves plan mode.
#[must_use]
pub fn access(
    options: &[ConfigOption],
    thread: Option<&AccessMode>,
    project: Option<&AccessMode>,
) -> Access {
    let modes = access_modes(options);
    let (chosen, source) = match (thread, project) {
        (Some(mode), _) => (Some(mode), AccessSource::Thread),
        (None, Some(mode)) => (Some(mode), AccessSource::Project),
        (None, None) => (None, AccessSource::Default),
    };
    let current = match chosen {
        Some(AccessMode {
            native: Some(native),
            ..
        }) => options
            .iter()
            .find(|option| option.id == native.option)
            .and_then(|option| match &option.value {
                OptionValue::Select(value) => modes.iter().find(|mode| {
                    mode.native
                        .as_ref()
                        .is_some_and(|it| it.option == native.option && it.value == *value)
                }),
                OptionValue::Boolean(_) => None,
            })
            .cloned()
            .or_else(|| chosen.cloned()),
        Some(mode) => Some(mode.clone()),
        None => None,
    };
    Access {
        current: current.unwrap_or_else(viz_full_access),
        source,
        modes,
    }
}

/// Checks a requested mode against the offered ones.
///
/// # Errors
/// [`Rejection::UnknownAccess`] when the thread's harness does not offer it.
pub fn choose(options: &[ConfigOption], mode: &AccessMode) -> Result<AccessMode, Rejection> {
    access_modes(options)
        .into_iter()
        .find(|offered| offered.kind == mode.kind && offered.native == mode.native)
        .ok_or(Rejection::UnknownAccess)
}

/// Full access answers permission requests at once, preferring to allow once.
/// Everything else waits for an answer.
#[must_use]
pub fn admit(access: AccessKind, body: &RequestBody) -> Admission {
    match (access, body) {
        (AccessKind::Full, RequestBody::Permission { options, .. }) => [
            PermissionKind::AllowOnce,
            PermissionKind::AllowAlways,
            PermissionKind::RejectOnce,
            PermissionKind::RejectAlways,
        ]
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind))
        .map_or(Admission::Wait, |option| {
            Admission::Answer(RequestAnswer::Permission {
                option: option.id.clone(),
            })
        }),
        _ => Admission::Wait,
    }
}

/// How a pending request stops waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Answer {
        by: AnsweredBy,
        answer: RequestAnswer,
    },
    Withdraw(Withdrawal),
}

/// Who asked: the request's thread and the thread that delegated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asker {
    pub parent: Option<ThreadId>,
}

/// The request's next state.
///
/// # Errors
/// When the request no longer waits, the answerer may not answer it, or the
/// answer does not fit the request.
pub fn decide(
    state: &RequestState,
    body: &RequestBody,
    asker: Asker,
    command: Command,
    now: i64,
) -> Result<RequestState, Rejection> {
    if *state != RequestState::Pending {
        return Err(Rejection::Settled);
    }
    let (by, answer) = match command {
        Command::Withdraw(reason) => return Ok(RequestState::Withdrawn { at: now, reason }),
        Command::Answer { by, answer } => (by, answer),
    };
    if let AnsweredBy::Agent(thread) = by
        && asker.parent != Some(thread)
    {
        return Err(Rejection::NotAllowed);
    }
    match (body, &answer) {
        (RequestBody::Permission { options, .. }, RequestAnswer::Permission { option }) => {
            if !options.iter().any(|offered| offered.id == *option) {
                return Err(Rejection::InvalidAnswer("no such option"));
            }
        }
        (RequestBody::Question { .. }, RequestAnswer::Decline) => {}
        (RequestBody::Question { schema, .. }, RequestAnswer::Accept { fields }) => {
            let properties = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            if fields
                .keys()
                .any(|key| !properties.is_some_and(|known| known.contains_key(key)))
            {
                return Err(Rejection::InvalidAnswer("a field is not in the form"));
            }
            let required = schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str);
            for key in required {
                if !fields.contains_key(key) {
                    return Err(Rejection::InvalidAnswer("a required field is missing"));
                }
            }
        }
        (RequestBody::Permission { .. }, _) => {
            return Err(Rejection::InvalidAnswer(
                "a permission takes one of its options",
            ));
        }
        (RequestBody::Question { .. }, RequestAnswer::Permission { .. }) => {
            return Err(Rejection::InvalidAnswer(
                "a question takes fields or a decline",
            ));
        }
    }
    Ok(RequestState::Answered {
        at: now,
        by,
        answer,
    })
}

/// Why a run's remaining requests stop waiting when it settles. A run is
/// interrupted only when the server stops.
#[must_use]
pub const fn on_settle(status: RunStatus) -> Withdrawal {
    match status {
        RunStatus::Cancelled => Withdrawal::Cancelled,
        RunStatus::Interrupted => Withdrawal::Restarted,
        RunStatus::Queued | RunStatus::Running | RunStatus::Completed | RunStatus::Failed => {
            Withdrawal::TurnEnded
        }
    }
}
