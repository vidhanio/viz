//! The picker's `316pt` model column: a `33pt` search header aligned with the
//! options header's hairline, then a scrolling list of `32pt` model rows with
//! sub-provider headings, "Show more" folds, favourite stars, `1..9` jump
//! hints, status banners and empty states.

use rune::{Align, El, TextEdit, col, icon, row, text};

use super::{FIELD, HEADER_HEIGHT, send};
use crate::{
    app::Desktop,
    model::{
        ListEmpty, ListItem, ModelRow, PickerColumn, PickerList, RailTab, status_note,
        status_usable,
    },
    theme::{ICON, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S20, S24, S28, S32, Theme},
    ui::{self, Tone},
    update::{ModelPickerMsg, Msg, model_picker::MODEL_LIST},
};

/// Ring target for rows when the keyboard is in another column: a key no
/// element has, so no ring draws.
const NO_RING: &str = "model-picker-no-ring";

/// The search header and model list.
pub(super) fn column(s: &Desktop, list: &PickerList<'_>, t: &Theme) -> El {
    let state = &s.model.composer.model_menu;
    let preview = list.preview(state).map(|row| row.model.clone());
    let keyboard = state.column == PickerColumn::Models;
    let search = ui::search_input(&s.widgets.model_query, "Search models…")
        .style(ui::SearchStyle::Plain)
        .input_key(FIELD)
        .label("Search models")
        .active_descendant(preview.as_ref().map(crate::model::ModelRef::row_key))
        .on_edit(|s: &mut Desktop| &mut s.widgets.model_query)
        .on_change(|s: &mut Desktop, tx| {
            let query = s.widgets.model_query.text.clone();
            s.dispatch(Msg::ModelPicker(ModelPickerMsg::Query(query)), tx);
        })
        .on_clear(|s: &mut Desktop, tx| {
            s.widgets.model_query = TextEdit::new("");
            s.widgets.model_query.focused = true;
            s.dispatch(Msg::ModelPicker(ModelPickerMsg::Query(String::new())), tx);
        })
        .on_key(super::on_key)
        .build(t);
    let header = col()
        .w_full()
        .h(HEADER_HEIGHT)
        .child(row().w_full().grow(1.0).px(S4).center_y().child(search))
        .child(ui::rule(t));
    let mut content = col()
        .key("model-rows")
        .role("listbox")
        .label("Models")
        .w_full()
        .px(S8)
        .py(S6)
        .instant_changes();
    if let Some(banner) = banner(list, t) {
        content = content.child(banner.mb(S6));
    }
    let favorites = !list.searching && list.tab == RailTab::Favorites;
    let mut index = 0;
    for item in &list.items {
        content = content.child(match item {
            ListItem::Header { key, label } => row()
                .key(key)
                .w_full()
                .h(S24)
                .px(S8)
                .center_y()
                .child(ui::section(label, t)),
            ListItem::Model(model) => {
                index += 1;
                let highlighted = preview.as_ref() == Some(&model.model);
                let hint = (favorites && index <= 9).then_some(index);
                model_row(
                    model,
                    highlighted,
                    keyboard,
                    hint,
                    list.searching || favorites,
                    t,
                )
            }
            ListItem::Fold {
                key,
                expanded,
                hidden,
            } => fold(key, *expanded, *hidden, t),
        });
    }
    if let Some(empty) = list.empty {
        content = content.child(empty_state(empty, &state.query, t));
    }
    col()
        .key("model-column")
        .grow(1.0)
        .shrink()
        .min_w(240.0)
        .h_full()
        .child(header)
        .child(ui::scroll_viewport(
            MODEL_LIST,
            ui::ScrollMode::Top,
            content,
            t,
        ))
}

fn banner(list: &PickerList<'_>, t: &Theme) -> Option<El> {
    let RailTab::Harness(id) = &list.tab else {
        return None;
    };
    if list.searching {
        return None;
    }
    let catalog = list.catalog(id)?;
    if let Some((title, detail)) = status_note(catalog.status) {
        let tone = if status_usable(catalog.status) {
            Tone::Warning
        } else {
            Tone::Danger
        };
        return Some(
            ui::status_banner(format!("model-status-{id}"), tone, title)
                .detail(detail)
                .build(t),
        );
    }
    (list.scope.locked && !catalog.own).then(|| {
        ui::status_banner(
            "model-locked",
            Tone::Info,
            format!("This thread runs on {}", list.scope.harness_name()),
        )
        .detail(format!("Start a new thread to use {}.", catalog.name))
        .build(t)
    })
}

fn model_row(
    model: &ModelRow<'_>,
    highlighted: bool,
    keyboard: bool,
    hint: Option<usize>,
    mixed: bool,
    t: &Theme,
) -> El {
    let key = model.model.row_key();
    let mut trailing = row().gap(S6).center_y();
    if model.selected {
        trailing = trailing.child(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.text_strong)
                .a11y_hidden(),
        );
    }
    if model.disabled {
        trailing = trailing.child(rune_kit::Badge::new("Unavailable").build(&t.k));
    }
    let star = if model.favorite {
        &ui::STAR_FILLED
    } else {
        &ui::STAR
    };
    let rest = if model.favorite || highlighted {
        1.0
    } else {
        0.0
    };
    trailing = trailing.child(
        row()
            .key(format!("{key}-star"))
            .role("button")
            .label(if model.favorite {
                "Remove from favorites"
            } else {
                "Add to favorites"
            })
            .focusable(false)
            .preserve_focus_on_press(true)
            .size(S20, S20)
            .center()
            .radius(RADIUS_CONTROL)
            .hover(t.hover_wash)
            .on_press_tx(send(ModelPickerMsg::ToggleFavorite(model.model.clone())))
            .child(
                icon(star, ICON_SMALL)
                    .color(if model.favorite { t.warning } else { t.faint })
                    .group_opacity(rest, 1.0)
                    .a11y_hidden(),
            ),
    );
    if let Some(hint) = hint {
        trailing = trailing.child(rune_kit::Keybind::new(&[hint.to_string()]).build(&t.k));
    }
    let mut item = ui::list_row(key, model.label)
        .leading(
            icon(ui::harness_icon(&model.model.harness), ICON).color(if mixed {
                t.text
            } else {
                t.icon_muted
            }),
        )
        .selected(model.selected)
        .highlighted(highlighted)
        .disabled(model.disabled)
        .focus_ring_for(if keyboard { FIELD } else { NO_RING })
        .trailing(trailing);
    if let Some(tag) = &model.tag {
        item = item.detail(tag.as_ref());
    }
    if !model.disabled {
        item = item.on_press_tx(send(ModelPickerMsg::Choose(model.model.clone())));
    }
    let hovered = model.model.clone();
    item.build(t)
        .preserve_focus_on_press(true)
        .description(model.harness_name)
        .on_hover(move |s: &mut Desktop| {
            if s.model.composer.model_menu.highlight.as_ref() != Some(&hovered) {
                s.dispatch_local(Msg::ModelPicker(ModelPickerMsg::Highlight(hovered.clone())));
            }
        })
}

fn fold(key: &str, expanded: bool, hidden: usize, t: &Theme) -> El {
    let label = if expanded {
        "Show less".to_owned()
    } else {
        format!("Show more ({hidden})")
    };
    row()
        .key(format!("model-fold-{key}"))
        .role("button")
        .label(&label)
        .expanded(expanded)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .h(S28)
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .hover(t.hover_wash)
        .hover_group()
        .on_press_tx(send(ModelPickerMsg::ToggleFold(key.to_owned())))
        .child(
            crate::motion::chevron(
                &ui::CHEVRON,
                ICON_SMALL,
                expanded,
                std::f32::consts::FRAC_PI_2,
            )
            .color(t.muted)
            .group_color(t.text)
            .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.small)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

fn empty_state(empty: ListEmpty, query: &str, t: &Theme) -> El {
    match empty {
        ListEmpty::Loading => ui::skeleton_rows("model-skeleton", 5, S24, true, t)
            .px(S8)
            .py(S4),
        ListEmpty::NoFavorites => col()
            .key("model-empty")
            .w_full()
            .py(S32)
            .px(S8)
            .gap(S6)
            .items_center()
            .child(icon(&ui::STAR, S20).color(t.faint).a11y_hidden())
            .child(
                text("No favorite models yet")
                    .font(t.ui_medium)
                    .color(t.text),
            )
            .child(
                text("Star models you use often to pin them here.")
                    .font(t.small)
                    .color(t.muted)
                    .wrap()
                    .align(Align::Center),
            ),
        ListEmpty::NoMatches | ListEmpty::NoModels => {
            let message = if empty == ListEmpty::NoMatches {
                format!("No models match \u{201c}{}\u{201d}.", query.trim())
            } else {
                "This harness offers no model choices.".to_owned()
            };
            text(message)
                .key("model-empty")
                .role("status")
                .font(t.small)
                .color(t.faint)
                .w_full()
                .py(S32)
                .wrap()
                .align(Align::Center)
        }
    }
}
