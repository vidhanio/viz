use rune::El;
use rune_kit::MenuItem;

use super::settings::{is_open, marked_picker, picker};
use crate::{
    app::{Desktop, NewThread},
    theme::{S8, Size, Theme},
    ui,
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
    if thread.sending {
        return bar
            .gap(S8)
            .child(ui::lead(ui::spinner(t), Size::Default.height()))
            .child(ui::section("Opening session…", t));
    }
    if s.harnesses.is_empty() {
        return bar.child(ui::section("No harness configured", t));
    }
    if s.harnesses.len() > 1 {
        return bar.child(harness(s, thread, t, layer));
    }
    bar
}

fn harness(s: &Desktop, thread: &NewThread, t: &Theme, layer: &mut Vec<El>) -> El {
    let label = "Choose harness";
    let name = thread
        .active_harness(&s.harnesses)
        .map(|runner| runner.name.as_str());
    // The chosen runner's mark leads the value inside its trigger, as in its
    // menu.
    let mut menu = name.map_or_else(
        || picker(s, HARNESS, label, None, t),
        |name| marked_picker(s, HARNESS, label, name, ui::harness_icon(name), t),
    );
    if is_open(s, HARNESS) {
        menu = ui::menu_items(
            menu,
            s.harnesses.iter().map(|harness| {
                let name = harness.name.clone();
                let selected = thread
                    .harness
                    .as_deref()
                    .map_or(harness.default, |picked| picked == name);
                let mut item = MenuItem::new(&name)
                    .icon(ui::harness_icon(&name))
                    .checked(selected);
                if harness.default {
                    item = item.badge("Default");
                }
                item.on_select_tx(move |s: &mut Desktop, tx| {
                    s.open_picker = None;
                    tx.focus("draft", false);
                    if !selected && let Some(thread) = &mut s.new_thread {
                        thread.harness = Some(name.clone());
                        thread.options.clear();
                    }
                })
            }),
        );
    }
    menu.build(&t.overlay, layer).min_w(0.0).shrink()
}
