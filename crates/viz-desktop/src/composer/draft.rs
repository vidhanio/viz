//! Draft harness picker and opening-session status in the composer toolbar.

use rune::El;
use rune_kit::MenuItem;

use super::settings::{is_open, marked_picker, picker};
use crate::{
    app::Desktop,
    model::NewThread,
    theme::{S8, Size, Theme},
    ui,
    update::{Msg, SelectionMsg},
};

const HARNESS: &str = "harness";

/// A draft's leading toolbar items before the runner's own settings: the
/// harness picker when there is a choice, or why there are no settings yet.
pub(super) fn toolbar(
    s: &Desktop,
    thread: &NewThread,
    bar: El,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let harnesses = s.model.harnesses();
    if thread.sending {
        return bar
            .gap(S8)
            .child(ui::lead(ui::spinner(t), Size::Default.height()))
            .child(ui::section("Opening session…", t));
    }
    if harnesses.is_empty() {
        return bar.child(ui::section("No harness configured", t));
    }
    if harnesses.len() > 1 {
        return bar.child(harness(s, thread, t, layer));
    }
    bar
}

fn harness(s: &Desktop, thread: &NewThread, t: &Theme, layer: &mut Vec<El>) -> El {
    let harnesses = s.model.harnesses();
    let label = "Choose harness";
    let runner = thread.active_harness(harnesses);
    let mut menu = runner.map_or_else(
        || picker(s, HARNESS, label, None, t),
        |runner| {
            marked_picker(
                s,
                HARNESS,
                label,
                &runner.name,
                ui::harness_icon(&runner.id),
                t,
            )
        },
    );
    if is_open(s, HARNESS) {
        menu = ui::menu_items(
            menu,
            harnesses.iter().map(|harness| {
                let id = harness.id.clone();
                let selected = thread
                    .harness
                    .as_deref()
                    .map_or(harness.default, |picked| picked == id);
                let mut item = MenuItem::new(&harness.name)
                    .icon(ui::harness_icon(&id))
                    .checked(selected);
                if harness.default {
                    item = item.badge("Default");
                }
                item.on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Selection(SelectionMsg::ChooseDraftHarness(id.clone())),
                        tx,
                    );
                })
            }),
        );
    }
    menu.build(&t.overlay, layer).min_w(0.0).shrink()
}
