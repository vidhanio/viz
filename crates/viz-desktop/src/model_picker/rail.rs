//! The picker's `44pt` harness rail: a Favorites star, a hairline, then one
//! `36pt` button per configured harness (default first). Unusable harnesses
//! sit at `0.40` opacity and explain their typed status in a tooltip. A
//! `3×20pt` accent pill glides between tabs on the right inner edge as one
//! shared element ([`motion::shared_pill`]).

use rune::{Color, El, Icon, col, el, icon, row};

use super::{FIELD, send};
use crate::{
    app::Desktop,
    model::{Catalog, PickerColumn, PickerList, RailTab, status_note, status_usable},
    motion,
    theme::{HAIRLINE, MODEL_PICKER_RAIL_WIDTH, RADIUS_CONTROL, RADIUS_PILL, S2, S4, S20, Theme},
    ui,
    update::ModelPickerMsg,
};

/// Rail button side.
const BUTTON: f32 = 36.0;
/// Active pill size.
const PILL_W: f32 = 3.0;
const PILL_H: f32 = S20;
/// Resting opacity of an inactive, usable harness.
const REST: f32 = 0.72;
/// Opacity of a harness that cannot run.
const UNUSABLE: f32 = 0.40;

/// The rail column.
pub(super) fn rail(s: &Desktop, list: &PickerList<'_>, t: &Theme) -> El {
    let keyboard = s.model.composer.model_menu.column == PickerColumn::Rail;
    let favorites = list.tab == RailTab::Favorites;
    let mut rail = col()
        .key("model-rail")
        .role("tablist")
        .label("Harnesses")
        .w(MODEL_PICKER_RAIL_WIDTH)
        .h_full()
        .padding(S4)
        .gap(S4)
        .fill(t.surface_recessed)
        .instant_changes()
        .child(button(
            RailTab::Favorites,
            if favorites {
                &ui::STAR_FILLED
            } else {
                &ui::STAR
            },
            18.0,
            "Favorites".to_owned(),
            favorites,
            true,
            keyboard,
            if favorites { t.warning } else { t.muted },
            t,
        ))
        .child(
            row()
                .w_full()
                .px(S4)
                .py(S2)
                .child(el().w_full().h(HAIRLINE).fill(t.hairline))
                .a11y_hidden(),
        );
    for catalog in &list.catalogs {
        let tab = RailTab::Harness(catalog.id.to_owned());
        let active = list.tab == tab;
        let glyph = ui::harness_icon(catalog.id);
        rail = rail.child(button(
            tab,
            glyph,
            S20,
            tooltip(catalog),
            active,
            status_usable(catalog.status),
            keyboard,
            t.text,
            t,
        ));
    }
    rail
}

fn tooltip(catalog: &Catalog<'_>) -> String {
    match status_note(catalog.status) {
        Some((title, detail)) if detail.is_empty() => format!("{} — {title}", catalog.name),
        Some((title, detail)) => format!("{} — {title}: {detail}", catalog.name),
        None => catalog.name.to_owned(),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a rail button's glyph, state flags and ink are independent"
)]
fn button(
    tab: RailTab,
    glyph: &'static Icon,
    size: f32,
    label: String,
    active: bool,
    usable: bool,
    keyboard: bool,
    ink: Color,
    t: &Theme,
) -> El {
    let opacity = match (usable, active) {
        (false, _) => UNUSABLE,
        (true, true) => 1.0,
        (true, false) => REST,
    };
    let mut item = row()
        .key(tab.key())
        .role("tab")
        .label(&label)
        .help(label)
        .checked(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .size(BUTTON, BUTTON)
        .center()
        .radius(RADIUS_CONTROL)
        .fill(if active {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover(t.hover_wash)
        .hover_group()
        .on_press_tx(send(ModelPickerMsg::Tab(tab)))
        .child(
            icon(glyph, size)
                .color(ink)
                .group_opacity(opacity, 1.0)
                .a11y_hidden(),
        );
    if active {
        item = item.child(motion::shared_pill(
            "model-rail-pill",
            el().key("model-rail-pill")
                .size(PILL_W, PILL_H)
                .radius(RADIUS_PILL)
                .fill(t.accent)
                .pin(None, Some((BUTTON - PILL_H) / 2.0), Some(-S4), None)
                .a11y_hidden(),
        ));
        if keyboard {
            item = item.focus_ring_for(FIELD, HAIRLINE, -HAIRLINE, t.accent);
        }
    }
    item
}
