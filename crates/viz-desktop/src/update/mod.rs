//! Pure state transitions: `fn update(&mut Model, Msg) -> Vec<Effect>`,
//! grouped per feature.
//!
//! Nothing under `update` imports Rune, Tokio, channels, or performs I/O.

mod composer;
mod composer_actions;
mod git;
pub use git::GitMsg;
mod home;
pub mod model_picker;
mod model_picker_keys;
mod palette;
mod projects;
pub mod requests;
mod selection;
mod server;
mod server_msg;
mod server_sync;
mod shell;
mod sidebar;
mod sidebar_triage;
mod timeline;

pub use self::{
    composer::{
        ComposerMsg, DRAFT_BRANCH_PICKER, DRAFT_BRANCH_SEARCH, HOME_ENV_PICKER,
        HOME_PROJECT_PICKER, HOME_PROJECT_SEARCH,
    },
    model_picker::ModelPickerMsg,
    model_picker_keys::PickerKey,
    palette::PaletteMsg,
    selection::SelectionMsg,
    server::ServerMsg,
    shell::ShellMsg,
    sidebar::{INLINE_RENAME_INPUT, PROJECT_FILTER_INPUT, SidebarMsg},
    timeline::TimelineMsg,
};
use crate::{effects::Effect, model::Model};

/// Keyboard navigation key translated at the view boundary so `update` stays
/// independent of Rune input types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    /// Move highlight up.
    Up,
    /// Move highlight down.
    Down,
    /// Activate highlighted choice.
    Enter,
    /// Complete highlighted choice.
    Tab,
    /// Dismiss active menu or completion.
    Escape,
}

/// Top-level message routed through [`update`].
#[derive(Debug)]
pub enum Msg {
    Requests(requests::RequestMsg),
    Git(GitMsg),
    /// Global lifecycle event from the background server or blocking platform
    /// worker (endpoint, connection status, initial handshake, local attach).
    Server(ServerMsg),
    /// Event scoped to a specific server WebSocket session; dropped when the
    /// session has been superseded by a reconnect or environment switch.
    Session {
        /// Session id that produced this message.
        session: crate::model::SessionId,
        /// Payload routed to [`server::update`] when `session` is still active.
        msg: Box<ServerMsg>,
    },
    /// Thread selection, navigation history, draft setup, or environment
    /// switch.
    Selection(SelectionMsg),
    /// Sidebar view mode, filter, menus, dialogs, and thread/project actions.
    Sidebar(SidebarMsg),
    /// Composer text, attachments, completions, and tasks dock.
    Composer(ComposerMsg),
    /// Unified model picker and access-mode popovers.
    ModelPicker(ModelPickerMsg),
    /// Timeline disclosure, pagination, links, and tool clock ticks.
    Timeline(TimelineMsg),
    /// Command palette modal actions.
    Palette(PaletteMsg),
    /// Window focus, geometry, shell slot toggles, preferences, and toasts.
    Shell(ShellMsg),
}

/// Pure reducer applying `msg` to `model` and returning side effects to run at
/// the shell edge.
#[must_use]
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    let mut effects = match msg {
        Msg::Requests(msg) => requests::update(model, msg),
        Msg::Git(msg) => git::update(model, msg),
        Msg::Server(msg) => server::update(model, msg),
        Msg::Session { session, msg } => {
            if session != model.connection.session {
                return Vec::new();
            }
            server::update(model, *msg)
        }
        Msg::Selection(msg) => selection::update(model, msg),
        Msg::Sidebar(msg) => sidebar::update(model, msg),
        Msg::Composer(msg) => composer::update(model, msg),
        Msg::ModelPicker(msg) => model_picker::update(model, msg),
        Msg::Timeline(msg) => timeline::update(model, msg),
        Msg::Palette(msg) => palette::update(model, msg),
        Msg::Shell(msg) => shell::update(model, msg),
    };
    git::reconcile(model, &mut effects);
    requests::reconcile(model);
    effects
}
