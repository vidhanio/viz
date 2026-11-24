//! Single exhaustive offline fallback derivation for [`ServerEffect`], shared
//! by the background reconnect loop and closed-channel runner fallback.

use rune::AppHandle;
use viz_client::protocol::ThreadId;

use super::{
    AttachmentSource, ServerEffect, platform,
    server::{dispatch_global, toast},
};
use crate::{
    app::Desktop,
    model::AttachmentTarget,
    update::{Msg, ServerMsg},
};

/// Derived offline disposition for one [`ServerEffect`].
pub(super) enum Offline {
    /// Update desired selected thread for the next connect.
    Select(ThreadId),
    /// Clear desired selected thread for the next connect.
    ClearSelection,
    /// Switch target environment and reconnect immediately.
    SwitchEnv {
        environment: String,
        thread: Option<ThreadId>,
    },
    /// Retry connecting immediately.
    Reconnect,
    /// Dispatch a typed failure/reset message into the reducer.
    Dispatch(Msg),
    /// Run a local filesystem/clipboard attachment read (works offline).
    Attach {
        scope: u64,
        target: AttachmentTarget,
        source: AttachmentSource,
    },
    /// Show a local OS notification (works offline).
    Notify {
        thread: ThreadId,
        title: String,
        body: &'static str,
    },
    /// Surface a generic offline toast.
    Toast,
    /// Silently ignore best-effort background fetches.
    Ignore,
}

/// Exhaustively derives how `command` behaves when no server session is
/// connected. No catch-all wildcard is permitted so every new [`ServerEffect`]
/// chooses an explicit offline contract.
#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive offline derivation over every ServerEffect variant"
)]
pub(super) fn derive(command: ServerEffect, active_env: &str) -> Offline {
    match command {
        ServerEffect::Select(thread) => Offline::Select(thread),
        ServerEffect::ClearSelection => Offline::ClearSelection,
        ServerEffect::Environment {
            environment,
            thread,
        } => Offline::SwitchEnv {
            environment,
            thread,
        },
        ServerEffect::Reconnect => Offline::Reconnect,
        ServerEffect::Create {
            draft, recovery, ..
        } => Offline::Dispatch(Msg::Server(ServerMsg::LaunchFailed {
            draft,
            recovery,
            title: "Offline".into(),
            detail: None,
        })),
        ServerEffect::LoadDraft(draft) => {
            Offline::Dispatch(Msg::Server(ServerMsg::DraftHarnessesLoaded {
                draft,
                result: None,
            }))
        }
        ServerEffect::SendInput { input, recovery } => {
            Offline::Dispatch(Msg::Server(ServerMsg::SendFailed {
                thread: input.thread,
                recovery,
                title: "Offline".into(),
                detail: None,
            }))
        }
        ServerEffect::ProjectPalette(request, _) => {
            Offline::Dispatch(Msg::Server(ServerMsg::ProjectPalette {
                environment: active_env.to_owned(),
                request,
                result: Box::new(Err("The server is offline.".into())),
            }))
        }
        ServerEffect::SearchThreads { request, .. } => {
            Offline::Dispatch(Msg::Server(ServerMsg::ThreadSearchResult {
                request,
                result: Err("Offline".into()),
            }))
        }
        ServerEffect::LoadEarlier { thread, before } => {
            Offline::Dispatch(Msg::Server(ServerMsg::EarlierLoaded {
                thread,
                before,
                page: None,
            }))
        }
        ServerEffect::ReadEntryChunk {
            thread,
            key,
            source,
        } => Offline::Dispatch(Msg::Server(ServerMsg::EntryChunkLoaded {
            thread,
            key,
            source,
            result: Err("Server disconnected".into()),
        })),
        ServerEffect::ReadFile { request, .. } => {
            Offline::Dispatch(Msg::Server(ServerMsg::FilePreviewLoaded {
                request,
                result: Err("Server disconnected".into()),
            }))
        }
        ServerEffect::ReadAttachment {
            thread,
            id,
            preview_request,
        } => Offline::Dispatch(Msg::Server(ServerMsg::ThreadAttachmentLoaded {
            thread,
            id,
            preview_request,
            result: Err("Server disconnected".into()),
        })),
        ServerEffect::Update { op_id, .. }
        | ServerEffect::Project {
            op_id: Some(op_id), ..
        } => Offline::Dispatch(Msg::Server(ServerMsg::MutationFailed {
            op_id,
            title: "Offline".into(),
            detail: "Server disconnected".into(),
        })),
        ServerEffect::Answer(answer) => Offline::Dispatch(Msg::Requests(
            crate::update::requests::RequestMsg::Finished(
                answer.request,
                Err("Server disconnected".into()),
            ),
        )),
        ServerEffect::Attach {
            scope,
            target,
            source,
        } => Offline::Attach {
            scope,
            target,
            source,
        },
        ServerEffect::Notify {
            thread,
            title,
            body,
        } => Offline::Notify {
            thread,
            title,
            body,
        },
        ServerEffect::SearchFiles { .. }
        | ServerEffect::SearchGitHub { .. }
        | ServerEffect::ReadProjectIcon { .. }
        | ServerEffect::LoadCheckouts(_)
        | ServerEffect::HarnessOptions { .. }
        | ServerEffect::Harnesses
        | ServerEffect::Git { .. } => Offline::Ignore,
        ServerEffect::CancelTask(_)
        | ServerEffect::Access { .. }
        | ServerEffect::Configure { .. }
        | ServerEffect::Retitle(_)
        | ServerEffect::Delete { .. }
        | ServerEffect::Move { .. }
        | ServerEffect::Abort(_)
        | ServerEffect::Project { op_id: None, .. }
        | ServerEffect::SteerQueued { .. }
        | ServerEffect::UpdateQueued { .. }
        | ServerEffect::RemoveQueued { .. }
        | ServerEffect::Fork { .. } => Offline::Toast,
    }
}

/// Handles a command while disconnected; returns `false` to reconnect now.
pub(super) fn handle(
    ui: &AppHandle<Desktop>,
    command: ServerEffect,
    selected: &mut Option<ThreadId>,
    target: &mut Option<String>,
) -> bool {
    let active_env = target.as_deref().unwrap_or(crate::model::DEFAULT_ENV_KEY);
    match derive(command, active_env) {
        Offline::Select(thread) => *selected = Some(thread),
        Offline::ClearSelection => *selected = None,
        Offline::SwitchEnv {
            environment,
            thread,
        } => {
            *selected = thread;
            *target = Some(environment);
            return false;
        }
        Offline::Reconnect => return false,
        Offline::Dispatch(Msg::Server(msg)) => dispatch_global(ui, msg),
        Offline::Dispatch(msg) => ui.update(move |s, tx| s.dispatch(msg, tx)),
        Offline::Attach {
            scope,
            target,
            source,
        } => platform::spawn_attach(ui, scope, target, source),
        Offline::Notify {
            thread,
            title,
            body,
        } => platform::spawn_notify(thread, &title, body),
        Offline::Toast => toast(ui, "Offline", "Server disconnected".into()),
        Offline::Ignore => {}
    }
    true
}
