//! View for the model-and-thinking picker menu. All model grouping, filtering,
//! and level transitions live in [`crate::model::picker`] and
//! [`crate::update::ComposerMsg`].

use rune::{
    Align, Color, Effect, El, Key, KeyEvent, MotionIntent, ScrollSpec, Side, Spring, SpringPair,
    Transition, Tx, col, icon, row, text,
};
use rune_kit::{TextInput, scrim};
use viz_client::protocol::ConfigOption;

use super::{settings, thinking};
use crate::{
    app::Desktop,
    model::{ModelEntry, ModelPicker, OptionOwner, model_groups, provider_name},
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_LIST_HEIGHT, MODEL_MENU_WIDTH, RADIUS_CARD, RADIUS_CONTROL, S4,
        S6, S8, S16, S24, Size, Theme,
    },
    ui,
    update::{ComposerMsg, Msg, NavKey},
};

/// The picker's `open_picker` value and key.
const KEY: &str = "model";
const FIELD: &str = "model-query";
const LIST: &str = "model-list";

/// The trigger and the menu while it is open.
pub(super) fn picker(s: &Desktop, picker: &ModelPicker<'_>, t: &Theme, layer: &mut Vec<El>) -> El {
    let full_label = picker.label();
    let open = settings::is_open(s, KEY);
    let current = picker.model_value().map(row_key);
    let model_text = picker.model_label();
    let effort = picker
        .level_selection()
        .map(|(levels, value, label, index, total)| {
            let tier = ui::Effort::classify(value, label.as_ref(), index, total);
            (levels, label, tier)
        });
    let rest_icon = if open { t.text } else { t.muted };
    let mut trigger = row()
        .key(format!("{KEY}-trigger"))
        .role("combobox")
        .label("Model")
        .a11y_value(full_label.as_ref())
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(Size::Default.height())
        .pl(S8)
        .pr(S4)
        .gap(S6)
        .center_y()
        .max_w(ui::MODEL_TRIGGER_MAX)
        .radius(RADIUS_CONTROL)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Composer(ComposerMsg::ToggleModelMenu {
                    current_row_key: current.clone(),
                }),
                tx,
            );
        });
    if let Some(model_name) = model_text {
        trigger = trigger.child(
            text(model_name)
                .font(t.ui_medium)
                .color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(model_name)
                .a11y_hidden(),
        );
    }
    if let Some((levels, effort_label, tier)) = effort {
        if model_text.is_some() {
            trigger = trigger.child(text("·").font(t.ui).color(t.faint).a11y_hidden());
        } else {
            trigger = trigger.child(
                text(ui::sentence_label(levels.name()))
                    .font(t.ui_medium)
                    .color(t.text)
                    .a11y_hidden(),
            );
        }
        trigger = trigger.child(ui::effort_glyph(tier, false, t)).child(
            text(effort_label)
                .font(t.ui)
                .color(if open { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        );
    } else if model_text.is_none() {
        trigger = trigger.child(
            text("Model")
                .font(t.ui_medium)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        );
    }
    trigger = trigger.child(
        icon(&ui::CHEVRON_OPEN, ICON_SMALL)
            .key("chevron")
            .animated_changes()
            .icon_rotation(if open { std::f32::consts::PI } else { 0.0 })
            .color(rest_icon)
            .group_color(t.text)
            .a11y_hidden(),
    );
    if open {
        menu(s, picker, t, layer);
    }
    trigger
}

fn close(s: &mut Desktop) {
    s.dispatch_local(Msg::Composer(ComposerMsg::CloseModelMenu));
}

fn menu(s: &Desktop, picker: &ModelPicker<'_>, t: &Theme, layer: &mut Vec<El>) {
    let k = &t.overlay;
    let max_width = s.model.panels.window_width - S16;
    let model_parts = picker.model.map(|model| models(s, picker, model, t));
    let loading_levels = s.model.composer.model_menu.loading_levels.is_some();
    let mut body = col()
        .key("model-menu")
        .role("dialog")
        .label("Model")
        .w(MODEL_MENU_WIDTH.min(max_width))
        .min_h(0.0)
        .radius(RADIUS_CARD)
        .fill(k.c.layer_01)
        .border(HAIRLINE, t.hairline)
        .focus_return(format!("{KEY}-trigger"))
        .modal(true, close);
    if let Some((field, list)) = model_parts {
        body = body
            .child(row().w_full().px(S4).pt(S4).instant_changes().child(field))
            .child(ui::rule(t))
            .child(list);
    }
    if picker.model.is_some() || picker.levels.is_some() {
        if picker.model.is_some() {
            body = body.child(ui::rule(t));
        }
        body = body.child(thinking::section(s, picker, loading_levels, t));
    }
    layer.push(scrim("model-scrim", close));
    let fade = Transition {
        insertion: Effect::IDENTITY,
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(rune::tok::motion::INSTANT, Spring::new(0.25, 0.0)),
        ..Transition::identity()
    };
    let body = body
        .anchored_fit(format!("{KEY}-trigger"), Side::Top, Align::Start, S6)
        .transition(fade)
        .motion_intent(MotionIntent::CUT)
        .token_motion(k.motion.overlay);
    layer.push(body);
}

/// The search field and matching models grouped by provider.
fn models(s: &Desktop, picker: &ModelPicker<'_>, model: &ConfigOption, t: &Theme) -> (El, El) {
    let current = picker.model_value();
    let groups = model_groups(model, &s.model.composer.model_menu.query);
    let values: Vec<String> = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|entry| entry.value.to_owned()))
        .collect();
    let current_visible =
        current.is_some_and(|current| values.iter().any(|value| value == current));
    let active = s
        .model
        .composer
        .model_menu
        .active
        .as_deref()
        .filter(|active| values.iter().any(|value| value == active))
        .or_else(|| current.filter(|_| current_visible))
        .or_else(|| values.first().map(String::as_str))
        .map(str::to_owned);
    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label("Models")
        .w_full()
        .max_h(MODEL_LIST_HEIGHT)
        .shrink()
        .min_h(0.0)
        .padding(S4)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S16)
        .scroll_margin(S16);
    for (heading, entries) in &groups {
        if let Some(heading) = heading {
            list = list.child(
                row()
                    .key(format!("provider-{heading}"))
                    .h(S24)
                    .px(S8)
                    .center_y()
                    .child(ui::section(&provider_name(heading), t)),
            );
        }
        for entry in entries {
            list = list.child(model_row(
                picker,
                entry,
                active.as_deref() == Some(entry.value),
                current == Some(entry.value),
                t,
            ));
        }
    }
    if values.is_empty() {
        list = list.child(
            text("No matching models")
                .role("status")
                .font(t.small)
                .color(t.faint)
                .px(S8)
                .py(S8),
        );
    }
    let owner = picker.target.owner();
    let id = model.id.clone();
    let committed = current.map(str::to_owned);
    let field = TextInput::new(&s.widgets.model_query)
        .label("Search models")
        .placeholder("Search models")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Default.height())
        .active_descendant(active.as_deref().map(row_key))
        .on_edit(|s: &mut Desktop| &mut s.widgets.model_query)
        .animate_edit_layout()
        .on_change(|s: &mut Desktop, tx| {
            let query = s.widgets.model_query.text.clone();
            s.dispatch(Msg::Composer(ComposerMsg::ModelQueryChanged(query)), tx);
        })
        .on_key(move |s, ev, tx| {
            on_key(
                s,
                ev,
                tx,
                &values,
                active.as_deref(),
                committed.as_deref(),
                owner,
                &id,
            )
        })
        .build(&t.overlay)
        .key(FIELD);
    (field, list)
}

fn row_key(value: &str) -> String {
    format!("model-{value}")
}

fn model_row(
    picker: &ModelPicker<'_>,
    entry: &ModelEntry<'_>,
    active: bool,
    current: bool,
    t: &Theme,
) -> El {
    let owner = picker.target.owner();
    let id = picker
        .model
        .map_or_else(String::new, |model| model.id.clone());
    let value = entry.value.to_owned();
    let mut item = row()
        .key(row_key(entry.value))
        .role("option")
        .label(entry.label)
        .checked(current)
        .focus_selected(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .h(Size::Default.height())
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if active {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover_group()
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Composer(ComposerMsg::ChooseModel {
                    owner,
                    option_id: id.clone(),
                    value: value.clone(),
                }),
                tx,
            );
        })
        .child(
            text(entry.label)
                .font(if active || current { t.ui_medium } else { t.ui })
                .color(if active || current { t.text } else { t.muted })
                .group_color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(entry.label)
                .a11y_hidden(),
        )
        .child(rune::spacer())
        .children(entry.detail.map(|detail| {
            text(detail)
                .font(t.mono_small)
                .color(t.faint)
                .group_color(t.muted)
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(detail)
                .a11y_hidden()
        }))
        .child(
            icon(&ui::CHECK, ICON_SMALL)
                .color(if current { t.accent_text } else { t.muted })
                .opacity(if current { 1.0 } else { 0.0 })
                .a11y_hidden(),
        );
    if let Some(detail) = entry.detail {
        item = item.description(detail);
    }
    item
}

#[expect(
    clippy::too_many_arguments,
    reason = "keyboard navigation needs the filtered list, highlight, committed value, and owner"
)]
fn on_key(
    s: &mut Desktop,
    ev: &KeyEvent,
    tx: &mut Tx,
    values: &[String],
    active: Option<&str>,
    committed: Option<&str>,
    owner: OptionOwner,
    id: &str,
) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.widgets.model_query.preedit().is_some() {
        return false;
    }
    let key = match ev.key {
        Key::Down => NavKey::Down,
        Key::Up => NavKey::Up,
        Key::Enter => NavKey::Enter,
        _ => return false,
    };
    if values.is_empty() || (key == NavKey::Enter && tx.key_repeat()) {
        return true;
    }
    s.dispatch(
        Msg::Composer(ComposerMsg::ModelKey {
            key,
            values: values.to_vec(),
            active: active.map(str::to_owned),
            committed: committed.map(str::to_owned),
            owner,
            option_id: id.to_owned(),
        }),
        tx,
    );
    true
}
