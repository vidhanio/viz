//! Per-thread composer, inline chips, attachments strip, tucked completion
//! drawers, dashed queued deck, and Home context strip.

mod actions;
pub mod attachments;
mod branch_picker;
pub mod codec;
mod completion;
pub mod context_strip;
mod draft;
mod editor;

pub use editor::CaretReveal;
pub mod markdown;
mod menus;
mod queued;
mod toolbar;

use rune::{El, col};

use crate::{
    app::Desktop,
    theme::{S16, Theme},
    ui,
};

/// Renders the bottom-docked composer stack (dashed queued deck, tucked
/// Question drawer, and composer card, with the completion popup in `layer`)
/// when an active thread is selected.
pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    m.selection.selected?;
    let mut column = col().w_full();
    if let Some(queued) = queued::stack(s, t) {
        column = column.child(queued);
    }
    if let Some(drawer) = crate::requests::question_drawer(s, t) {
        column = column.child(drawer);
    }
    menus::popup(s, t, layer);
    let card = editor::card(s, false, t, layer);
    Some(ui::reading(column.child(card)).key("composer").pb(S16))
}

/// Renders the Home screen centered composer card, its completion popup, and
/// bottom Context Strip.
pub fn home_composer(s: &Desktop, lane_width: f32, t: &Theme, layer: &mut Vec<El>) -> El {
    let column = col().key("home-composer").w_full();
    menus::popup(s, t, layer);
    let card = editor::card(s, true, t, layer);
    let strip = context_strip::home_strip(s, lane_width, t, layer);
    column.child(card).child(strip)
}
