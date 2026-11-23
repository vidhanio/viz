//! Composer footer controls for draft status and model selection.

use rune::{El, row};

use super::draft;
use crate::{
    app::Desktop,
    model_picker,
    theme::{S2, Size, Theme},
};

/// Renders draft status and the model trigger.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let bar = row()
        .key("settings")
        .h(Size::Default.height())
        .gap(S2)
        .center_y();
    if !m.connected() {
        return bar;
    }
    let bar = match &m.selection.new_thread {
        Some(thread) => draft::status(s, thread, bar, t),
        None => bar,
    };
    bar.children(model_picker::trigger_button(s, t, layer))
}
