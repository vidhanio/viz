//! Offline fallback handler for [`ServerEffect`] commands received while
//! disconnected from the server.

use rune::AppHandle;
use viz_client::protocol::ThreadId;

use super::{
    ServerEffect, platform,
    server::{dispatch_ui, toast},
};
use crate::{
    app::Desktop,
    update::{Msg, ServerMsg},
};

/// Handles a command while disconnected; returns `false` to reconnect now.
#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive offline fallback match over ServerEffect variants"
)]
pub(super) fn handle(
    ui: &AppHandle<Desktop>,
    command: ServerEffect,
    selected: &mut Option<ThreadId>,
    target: &mut Option<String>,
) -> bool {
    match command {
        ServerEffect::Select(thread) => *selected = Some(thread),
        ServerEffect::ClearSelection => *selected = None,
        ServerEffect::Environment {
            environment,
            thread,
        } => {
            *selected = thread;
            *target = Some(environment);
            return false;
        }
        ServerEffect::Create {
            draft, recovery, ..
        } => {
            dispatch_ui(
                ui,
                ServerMsg::LaunchFailed {
                    draft,
                    recovery,
                    title: "Offline".into(),
                    detail: None,
                },
            );
        }
        ServerEffect::LoadDraft(draft) => {
            dispatch_ui(
                ui,
                ServerMsg::DraftHarnessesLoaded {
                    draft,
                    result: None,
                },
            );
        }
        ServerEffect::SendInput { input, recovery } => {
            dispatch_ui(
                ui,
                ServerMsg::SendFailed {
                    thread: input.thread,
                    recovery,
                    title: "Offline".into(),
                    detail: None,
                },
            );
        }
        ServerEffect::SearchFiles { .. }
        | ServerEffect::SearchGitHub { .. }
        | ServerEffect::ReadProjectIcon { .. } => {}
        ServerEffect::ProjectPalette(request, _) => ui.update(move |s, tx| {
            s.dispatch(
                Msg::Server(ServerMsg::ProjectPalette {
                    environment: s.model.active_env_key().to_owned(),
                    request,
                    result: Box::new(Err("The server is offline.".into())),
                }),
                tx,
            );
        }),
        ServerEffect::SearchThreads { request, .. } => {
            dispatch_ui(
                ui,
                ServerMsg::ThreadSearchResult {
                    request,
                    result: Err("Offline".into()),
                },
            );
        }
        ServerEffect::LoadEarlier { thread, .. } => {
            dispatch_ui(
                ui,
                ServerMsg::EarlierLoaded {
                    thread,
                    transcript: None,
                },
            );
        }
        ServerEffect::ReadEntryChunk {
            thread,
            key,
            source,
        } => {
            dispatch_ui(
                ui,
                ServerMsg::EntryChunkLoaded {
                    thread,
                    key,
                    source,
                    result: Err("Server disconnected".into()),
                },
            );
        }
        ServerEffect::Update { op_id, .. }
        | ServerEffect::Project {
            op_id: Some(op_id), ..
        } => {
            dispatch_ui(
                ui,
                ServerMsg::MutationFailed {
                    op_id,
                    title: "Offline".into(),
                    detail: "Server disconnected".into(),
                },
            );
        }
        ServerEffect::Attach {
            scope,
            target,
            source,
        } => platform::spawn_attach(ui, scope, target, source),
        ServerEffect::Notify { title, body } => platform::spawn_notify(&title, body),
        ServerEffect::Answer(answer) => ui.update(move |app, tx| {
            app.dispatch(
                Msg::Requests(crate::update::requests::RequestMsg::Finished(
                    answer.request,
                    Err("Server disconnected".into()),
                )),
                tx,
            );
        }),
        ServerEffect::Reconnect => return false,
        _ => toast(ui, "Offline", "Server disconnected".into()),
    }
    true
}
