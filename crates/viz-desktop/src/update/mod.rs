//! Pure state transitions: `fn update(&mut Model, Msg) -> Vec<Effect>`,
//! grouped per feature.
//!
//! Nothing under `update` imports Rune, Tokio, channels, or performs I/O.

mod composer;
mod git;
pub use git::GitMsg;
pub mod model_picker;
mod model_picker_keys;
mod palette;
pub mod requests;
mod selection;
mod server;
mod server_msg;
mod server_sync;
mod shell;
mod sidebar;
mod sidebar_triage;
pub mod terminal;
mod timeline;

pub use self::{
    composer::ComposerMsg,
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
    Terminal(terminal::TerminalMsg),
    Requests(requests::RequestMsg),
    Git(GitMsg),
    /// Event arriving from the background server or blocking platform worker.
    Server(ServerMsg),
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
        Msg::Terminal(msg) => terminal::update(model, msg),
        Msg::Requests(msg) => requests::update(model, msg),
        Msg::Git(msg) => git::update(model, msg),
        Msg::Server(msg) => server::update(model, msg),
        Msg::Selection(msg) => selection::update(model, msg),
        Msg::Sidebar(msg) => sidebar::update(model, msg),
        Msg::Composer(msg) => composer::update(model, msg),
        Msg::ModelPicker(msg) => model_picker::update(model, msg),
        Msg::Timeline(msg) => timeline::update(model, msg),
        Msg::Palette(msg) => palette::update(model, msg),
        Msg::Shell(msg) => shell::update(model, msg),
    };
    git::reconcile(model, &mut effects);
    terminal::reconcile(model, &mut effects);
    requests::reconcile(model);
    effects
}
