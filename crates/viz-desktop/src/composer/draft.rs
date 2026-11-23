//! A draft's status in the composer toolbar before the runner's own controls.

use rune::El;

use crate::{
    app::Desktop,
    model::NewThread,
    theme::{S8, Size, Theme},
    ui,
};

/// Why a draft has no model controls yet: its session is opening, or no
/// harness is configured. Harness choice lives in the model picker's rail.
pub(super) fn status(s: &Desktop, thread: &NewThread, bar: El, t: &Theme) -> El {
    if thread.sending {
        return bar
            .gap(S8)
            .child(ui::lead(ui::spinner(t), Size::Default.height()))
            .child(ui::section("Opening session…", t));
    }
    if s.model.harnesses().is_empty() {
        return bar.child(ui::section("No harness configured", t));
    }
    bar
}
