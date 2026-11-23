//! Unified model picker: one docked popover above the composer that picks the
//! harness, model, reasoning effort, extra ACP options, speed and access mode.
//!
//! Layout is a `44pt` harness rail ([`rail`]), a `316pt` searchable model
//! list ([`models`]), a `192pt` effort-and-options column naming the model it
//! configures ([`options`]), and a `44pt` footer with the speed tier and the
//! access-mode trigger ([`access`]). The surface is flat `surface_raised` with
//! a `1pt` `border_overlay` hairline and no shadow (`decisions.md` #8).
//!
//! Keyboard focus never leaves the search field: the model's
//! [`PickerColumn`] decides where arrows, Enter, Tab and type-ahead go, and
//! rows in other columns draw their ring with `focus_ring_for` the field.
//! State and derivations live in [`crate::model::model_menu`] and
//! [`crate::model::model_options`]; transitions in
//! [`crate::update::ModelPickerMsg`].
//!
//! [`trigger_button`] (composer footer) and [`access::access_trigger`] (Home
//! context strip) are the public entry points.

pub mod access;
mod models;
mod options;
mod rail;

use rune::{El, Key, KeyEvent, Tx, col, icon, row, text};
use rune_kit::{Placement, scrim};

use crate::{
    app::Desktop,
    model::{
        MODEL_PICKER, PickerColumn, context_badge, effort_column, picker_list, picker_scope, speed,
    },
    motion,
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_PICKER_BODY_HEIGHT, MODEL_PICKER_OPTIONS_WIDTH,
        MODEL_PICKER_RAIL_WIDTH, RADIUS_CONTROL, RADIUS_XS, S4, S6, S8, S12, S16, Theme,
    },
    ui,
    update::{ModelPickerMsg, Msg, PickerKey},
};

/// Element key of the trigger the popover anchors to.
pub const TRIGGER: &str = "model-trigger";
/// Search field key that list and option rows ring for.
const FIELD: &str = crate::update::model_picker::MODEL_FIELD;
/// Model list column width.
const MODELS_WIDTH: f32 = 316.0;
/// Footer bar height.
const FOOTER_HEIGHT: f32 = 44.0;
/// Header row height shared by the search field and the options header.
const HEADER_HEIGHT: f32 = 33.0;
/// Trigger height.
const TRIGGER_HEIGHT: f32 = 28.0;
/// Widest model name in the trigger.
const TRIGGER_NAME_MAX: f32 = 224.0;

/// A press handler dispatching `msg`.
fn send(msg: ModelPickerMsg) -> impl Fn(&mut Desktop, &mut Tx) + Clone + 'static {
    move |s: &mut Desktop, tx: &mut Tx| s.dispatch(Msg::ModelPicker(msg.clone()), tx)
}

fn close(s: &mut Desktop) {
    s.dispatch_local(Msg::ModelPicker(ModelPickerMsg::Close));
}

/// Whether the model picker is open.
#[must_use]
pub fn is_open(s: &Desktop) -> bool {
    s.model.composer.open_picker.as_deref() == Some(MODEL_PICKER)
}

/// `Cmd/Ctrl+/` toggles the picker from anywhere in the chat pane.
pub fn shortcut(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !ev.is_command('/') || ev.mods.shift || ev.mods.alt {
        return false;
    }
    if picker_scope(&s.model).is_none() {
        return false;
    }
    s.dispatch(Msg::ModelPicker(ModelPickerMsg::Toggle), tx);
    true
}

/// The composer footer's model trigger: harness mark, model short name,
/// effort suffix, fast bolt and context badge. Mounts the popover into `layer`
/// while open.
pub fn trigger_button(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let scope = picker_scope(&s.model)?;
    let picker = scope.picker();
    let open = is_open(s);
    let name = picker
        .as_ref()
        .and_then(crate::model::ModelPicker::model_label)
        .unwrap_or_else(|| scope.harness_name());
    let label = picker
        .as_ref()
        .map_or_else(|| name.to_owned(), |picker| picker.label().into_owned());
    let mut trigger = row()
        .key(TRIGGER)
        .role("combobox")
        .label("Model")
        .a11y_value(label)
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(TRIGGER_HEIGHT)
        .px(S8)
        .gap(S6)
        .center_y()
        .min_w(0.0)
        .shrink()
        .radius(RADIUS_CONTROL)
        .fill(if open {
            t.selected
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(t.hover_wash)
        .on_press_tx(send(ModelPickerMsg::Toggle))
        .child(
            icon(ui::harness_icon(scope.harness), ICON_SMALL + S4)
                .color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(name)
                .font(t.ui_medium)
                .color(t.text_strong)
                .max_w(TRIGGER_NAME_MAX)
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(name)
                .a11y_hidden(),
        );
    if let Some((_, value, level, index, total)) = picker
        .as_ref()
        .and_then(crate::model::ModelPicker::level_selection)
    {
        let tier = ui::Effort::classify(value, &level, index, total);
        trigger = trigger.child(ui::effort_glyph(tier, false, t)).child(
            text(level)
                .font(t.ui)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        );
    }
    if speed(&scope).is_some_and(|speed| speed.is_fast) {
        trigger = trigger.child(icon(&ui::ZAP, ICON_SMALL).color(t.warning).a11y_hidden());
    }
    if let Some(badge) = context_badge(&scope) {
        trigger = trigger.child(
            text(badge)
                .font(t.mono_micro)
                .color(t.muted)
                .px(S4)
                .radius(RADIUS_XS)
                .fill(t.chip_fill)
                .a11y_hidden(),
        );
    }
    let trigger = ui::with_tooltip(trigger, "Model", Some(&ui::shortcut("/")));
    if open {
        popover(s, t, layer);
    }
    Some(trigger)
}

fn popover(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let Some(list) = picker_list(&s.model) else {
        return;
    };
    let state = &s.model.composer.model_menu;
    let column = effort_column(&list, state);
    let width = (MODEL_PICKER_RAIL_WIDTH + MODELS_WIDTH + MODEL_PICKER_OPTIONS_WIDTH)
        .min(s.model.panels.window_width - S16);
    let mut body = row()
        .key("model-picker-body")
        .w_full()
        .h(MODEL_PICKER_BODY_HEIGHT)
        .min_h(0.0);
    if !list.searching {
        body = body.child(rail::rail(s, &list, t)).child(divider(t));
    }
    body = body
        .child(models::column(s, &list, t))
        .child(divider(t))
        .child(options::column(s, &column, t));
    let footer = row()
        .key("model-picker-footer")
        .w_full()
        .h(FOOTER_HEIGHT)
        .px(S12)
        .gap(S8)
        .center_y()
        .children(speed(&list.scope).map(|speed| {
            ui::segmented(&["Standard", "Fast"], Some(usize::from(speed.is_fast)))
                .label("Speed")
                .on_pick(|s: &mut Desktop, index, tx| {
                    s.dispatch(Msg::ModelPicker(ModelPickerMsg::SetFast(index == 1)), tx);
                })
                .build(t)
        }))
        .child(rune::spacer())
        .children(access::footer_trigger(s, t));
    let content = col()
        .w_full()
        .role("dialog")
        .label("Model picker")
        .child(body)
        .child(ui::rule(t))
        .child(footer);
    let frame = ui::popover_frame("model-picker", width, content, t)
        .focus_return(TRIGGER)
        .paint_order(40);
    layer.push(scrim("model-scrim", close));
    layer.push(motion::anchored_popover(
        frame,
        TRIGGER,
        Placement::TopStart,
        S8,
    ));
    if let Some(submenu) = options::submenu(s, &column, t) {
        layer.push(submenu);
    }
    if state.access_open {
        layer.extend(access::popover(
            s,
            access::FOOTER_TRIGGER,
            Placement::TopEnd,
            t,
        ));
    }
}

/// Translates a key in the search field into a picker key, leaving text
/// editing keys to the field.
fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let edit = &s.widgets.model_query;
    if edit.preedit().is_some() {
        return false;
    }
    let state = &s.model.composer.model_menu;
    let typing = state.column == PickerColumn::Models && state.submenu.is_none();
    let caret = edit.anchor.min(edit.focus);
    let picked = match (&ev.key, ev.mods.ctrl || ev.mods.cmd || ev.mods.alt) {
        (Key::Command(ch), _) if ch.is_ascii_digit() && *ch != '0' => {
            Some(PickerKey::Digit(*ch as u8 - b'0'))
        }
        (_, true) => None,
        (Key::Up, _) => Some(PickerKey::Up),
        (Key::Down, _) => Some(PickerKey::Down),
        (Key::Left, _) => Some(PickerKey::Left {
            edge: edit.text.is_empty() || (caret == 0 && edit.anchor == edit.focus),
        }),
        (Key::Right, _) => Some(PickerKey::Right {
            edge: edit.anchor == edit.focus && edit.focus == edit.text.len(),
        }),
        (Key::Enter, _) if !tx.key_repeat() => Some(PickerKey::Enter),
        (Key::Enter, _) => return true,
        (Key::Tab, _) => Some(PickerKey::Tab {
            back: ev.mods.shift,
        }),
        (Key::Escape, _) => Some(PickerKey::Escape),
        (Key::Text(text), _) if !typing => match text.as_str() {
            " " => Some(PickerKey::Space),
            other => other.chars().next().map(PickerKey::Char),
        },
        (Key::Text(text), _)
            if edit.text.is_empty()
                && s.model.composer.model_menu.tab == Some(crate::model::RailTab::Favorites) =>
        {
            text.chars()
                .next()
                .and_then(|ch| ch.to_digit(10))
                .and_then(|digit| u8::try_from(digit).ok())
                .filter(|digit| *digit > 0)
                .map(PickerKey::Digit)
        }
        _ => None,
    };
    let Some(key) = picked else {
        return !typing && matches!(ev.key, Key::Backspace | Key::Delete);
    };
    s.dispatch(Msg::ModelPicker(ModelPickerMsg::Key(key)), tx);
    true
}

/// A `1pt` vertical hairline between columns.
fn divider(t: &Theme) -> El {
    rune::el()
        .w(HAIRLINE)
        .h_full()
        .fill(t.hairline)
        .a11y_hidden()
}
