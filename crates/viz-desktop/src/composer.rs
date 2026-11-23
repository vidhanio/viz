//! Per-thread composer, inline chips, attachments strip, tucked completion
//! drawers, dashed queued deck, Tasks Dock, and Home context strip.

mod actions;
pub mod attachments;
mod branch_picker;
mod completion;
pub mod context_strip;
mod draft;
mod editor;
mod menus;
mod queued;
mod task_row;
mod tasks;
mod toolbar;

use rune::{El, col};
pub use tasks::DOCK_KEY as TASKS_DOCK_KEY;

use crate::{
    app::Desktop,
    theme::{S16, Theme},
    ui,
};

/// Renders the bottom-docked composer stack (dashed queued deck, Tasks Dock,
/// tucked Question or `@`/`/`/`$`/`#` completion drawer, and composer card)
/// when an active thread is selected.
pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    m.selection.selected?;
    let mut column = col().w_full();
    if let Some(queued) = queued::stack(s, t) {
        column = column.child(queued);
    }
    if let Some(dock) = tasks::dock(s, t) {
        column = column.child(dock);
    }
    if let Some(drawer) = crate::requests::question_drawer(s, t).or_else(|| menus::drawer(s, t)) {
        column = column.child(drawer);
    }
    let card = editor::card(s, false, t, layer);
    Some(ui::reading(column.child(card)).key("composer").pb(S16))
}

/// Renders the Home screen centered composer card with tucked completion
/// drawer and bottom Context Strip.
pub fn home_composer(s: &Desktop, lane_width: f32, t: &Theme, layer: &mut Vec<El>) -> El {
    let mut column = col().key("home-composer").w_full();
    if let Some(drawer) = menus::drawer(s, t) {
        column = column.child(drawer);
    }
    let card = editor::card(s, true, t, layer);
    let strip = context_strip::home_strip(s, lane_width, t, layer);
    column.child(card).child(strip)
}
