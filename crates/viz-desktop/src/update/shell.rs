//! Pure transitions for window lifecycle, shell panel slots, and persisted
//! client preferences.

use viz_client::protocol::TriageAction;

use super::selection::apply_thread_change;
use crate::{
    effects::{Effect, ThreadChange},
    model::{Model, Prefs},
};

/// Shell-level window, pane, and preferences messages.
#[derive(Debug)]
pub enum ShellMsg {
    /// Window gained or lost OS focus; regaining focus marks an open unread
    /// thread read.
    WindowFocus(bool),
    /// Window width changed after layout/draw.
    WindowResized(f32, f32),
    /// Shell's `PaneState` for the sidebar changed via drag or header toggle.
    SidebarPaneChanged {
        /// Whether the sidebar is open.
        open: bool,
        /// Sidebar width in points.
        width: f32,
    },
    /// Restore loaded client preferences at launch.
    ApplyPrefs(Prefs),
    /// Emit a user-visible toast (e.g., invalid desktop config on startup).
    ShowToast {
        /// Toast title.
        title: String,
        /// Optional toast detail.
        detail: Option<String>,
    },
}

pub(super) fn update(model: &mut Model, msg: ShellMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        ShellMsg::WindowFocus(focused) => {
            model.disclosure.focused = focused;
            if focused
                && model.connected()
                && let Some(id) = model.selection.selected.filter(|&id| {
                    model
                        .summary(id)
                        .is_some_and(|thread| thread.lifecycle.unread)
                })
            {
                apply_thread_change(
                    model,
                    id,
                    ThreadChange::Triage(TriageAction::MarkRead),
                    model.disclosure.tool_clock,
                    &mut effects,
                );
            }
        }
        ShellMsg::WindowResized(width, height) => {
            model.panels.window_width = width;
            model.panels.window_height = height;
        }
        ShellMsg::SidebarPaneChanged { open, width } => {
            let changed = model.panels.sidebar.open != open
                || (model.panels.sidebar.width - width).abs() >= 0.5;
            if changed {
                model.panels.sidebar.open = open;
                model.panels.sidebar.width = width
                    .round()
                    .clamp(model.panels.sidebar.min, model.panels.sidebar.max);
                effects.push(Effect::SavePrefs(model.to_prefs()));
            }
        }
        ShellMsg::ApplyPrefs(prefs) => {
            model.apply_prefs(&prefs);
            effects.push(Effect::SyncSidebarPane {
                open: Some(model.panels.sidebar.open),
                width: Some(model.panels.sidebar.width),
            });
            effects.push(Effect::SyncDiffPane {
                open: Some(model.panels.diff.open),
                width: Some(model.panels.diff.width),
            });
        }
        ShellMsg::ShowToast { title, detail } => {
            effects.push(Effect::Toast { title, detail });
        }
    }
    effects
}
