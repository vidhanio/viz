//! Fixed-height thinking-level footer inside the model-and-thinking picker.

use std::borrow::Cow;

use rune::{Color, El, col, row, text};

use crate::{
    app::Desktop,
    model::{ModelLevels, ModelPicker},
    theme::{HAIRLINE, S4, S6, S8, Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Fixed-height thinking footer: shows a loading row while fetching a model's
/// thinking levels, a quiet placeholder when a model has none, or filled tier
/// chips on one line.
pub(super) fn section(s: &Desktop, picker: &ModelPicker<'_>, loading: bool, t: &Theme) -> El {
    let heading = picker.levels.map_or(Cow::Borrowed("Thinking"), |levels| {
        ui::sentence_label(levels.name())
    });
    let content = if loading {
        row()
            .key("levels-loading")
            .w_full()
            .h(Size::Small.height())
            .gap(S6)
            .center_y()
            .child(ui::spinner(t))
            .child(text("Loading levels…").font(t.small).color(t.faint))
    } else if let Some(levels) = picker.levels {
        chips(s, picker, levels, &heading, t)
    } else {
        row()
            .key("levels-empty")
            .w_full()
            .h(Size::Small.height())
            .center_y()
            .child(text("No thinking levels").font(t.small).color(t.faint))
    };
    col()
        .key("model-thinking")
        .w_full()
        .px(S8)
        .pt(S6)
        .pb(S8)
        .gap(S4)
        .child(ui::section(&heading, t))
        .child(content)
}

fn chips(
    s: &Desktop,
    picker: &ModelPicker<'_>,
    levels: ModelLevels<'_>,
    name: &str,
    t: &Theme,
) -> El {
    let id = levels.id();
    let selected = s
        .model
        .composer
        .model_menu
        .chosen
        .as_ref()
        .filter(|(option, value)| option == id && levels.has(value))
        .map(|(_, value)| value.as_str())
        .or_else(|| levels.value(picker.target));
    let owner = picker.target.owner();
    let choices: Vec<_> = levels.choices().collect();
    let total = choices.len().max(1);
    let mut chips = row()
        .key("levels")
        .role("radiogroup")
        .label(name)
        .w_full()
        .h(Size::Small.height())
        .gap(S4)
        .focus_group()
        .scroll_x();
    for (index, (value, choice)) in choices.into_iter().enumerate() {
        let on = selected == Some(value);
        let label = levels.choice_label(value, choice);
        let tier = ui::Effort::classify(value, label.as_ref(), index, total);
        let key = format!("level-{value}");
        let id = id.to_owned();
        let value = value.to_owned();
        chips = chips.child(
            row()
                .key(key)
                .role("radio")
                .label(label.as_ref())
                .checked(on)
                .h(Size::Small.height())
                .px(S8)
                .gap(S4)
                .center()
                .radius(Size::Small.radius())
                .fill(if on { t.accent } else { Color::TRANSPARENT })
                .hover_group()
                .press_scale(0.97)
                .focusable(true)
                .focus_selected(on)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    if !on {
                        s.dispatch(
                            Msg::Composer(ComposerMsg::ChooseLevel {
                                owner,
                                option_id: id.clone(),
                                value: value.clone(),
                            }),
                            tx,
                        );
                    }
                })
                .child(ui::effort_glyph(tier, on, t))
                .child(
                    text(label)
                        .font(t.small_medium)
                        .color(if on { t.on_accent } else { t.muted })
                        .group_color(if on { t.on_accent } else { t.text })
                        .a11y_hidden(),
                ),
        );
    }
    chips
}
