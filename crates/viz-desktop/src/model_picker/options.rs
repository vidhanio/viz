//! The picker's `192pt` options column and its side submenu.
//!
//! The `33pt` header reads "Reasoning" (or "Options") and names the model the
//! column configures: always the previewed model, never a stale one
//! (`decisions.md` #9). Below sit `32pt` effort radio rows, then a hairline and
//! the harness's extra ACP options: switches toggle in place, selects open a
//! non-modal side submenu that `ArrowLeft`/`Escape` close on their own.

use rune::{Color, El, col, icon, row, text};
use rune_kit::Placement;

use super::{FIELD, HEADER_HEIGHT, send};
use crate::{
    app::Desktop,
    model::{EffortColumn, EffortLevel, ExtraOption, PickerColumn, sentence_label},
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_PICKER_OPTIONS_WIDTH, RADIUS_CONTROL, S2, S4, S6, S8, S10, S12,
        S16, S32, S40, Theme,
    },
    ui,
    update::{ModelPickerMsg, Msg},
};

/// Side submenu width.
const SUBMENU_WIDTH: f32 = 240.0;

fn option_key(id: &str) -> String {
    format!("model-option-{id}")
}

/// The options column for `column`'s model.
pub(super) fn column(s: &Desktop, column: &EffortColumn<'_>, t: &Theme) -> El {
    let state = &s.model.composer.model_menu;
    let keyboard = state.column == PickerColumn::Options && state.submenu.is_none();
    let cursor = keyboard.then_some(state.option_cursor);
    let title = if column.levels.is_empty() && !column.extras.is_empty() {
        "Options"
    } else {
        "Reasoning"
    };
    let header = row()
        .key("model-options-header")
        .w_full()
        .h(HEADER_HEIGHT - HAIRLINE)
        .px(S12)
        .gap(S8)
        .center_y()
        .child(text(title).font(t.small_medium).color(t.muted))
        .child(rune::spacer())
        .children(column.model_label.map(|name| {
            text(name)
                .key("model-options-model")
                .font(t.micro)
                .color(t.faint)
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(name)
        }));
    let mut body = col()
        .key("model-options")
        .role("radiogroup")
        .label(title)
        .w_full()
        .px(S8)
        .pt(S10)
        .pb(S6)
        .gap(S2)
        .instant_changes();
    if column.loading {
        body = body.child(
            row()
                .key("levels-loading")
                .h(S32)
                .px(S8)
                .gap(S6)
                .center_y()
                .child(ui::spinner(t))
                .child(text("Loading levels…").font(t.small).color(t.faint)),
        );
    } else if column.levels.is_empty() {
        body = body.child(
            text("No effort levels")
                .key("levels-empty")
                .font(t.small)
                .color(t.faint)
                .px(S4)
                .py(S16),
        );
    }
    for level in column.levels.iter().filter(|_| !column.loading) {
        body = body.child(level_row(column, level, cursor == Some(level.index), t));
    }
    if !column.extras.is_empty() {
        body = body.child(ui::rule(t).mt(S4).mb(S4));
    }
    let offset = column.levels.len();
    for (index, extra) in column.extras.iter().enumerate() {
        let open = state.submenu.as_deref() == Some(extra.option().id.as_str());
        body = body.child(extra_row(
            *extra,
            cursor == Some(offset + index) || open,
            column.editable,
            t,
        ));
    }
    col()
        .key("model-options-column")
        .w(MODEL_PICKER_OPTIONS_WIDTH)
        .shrink()
        .h_full()
        .child(header)
        .child(ui::rule(t))
        .child(ui::scroll_viewport(
            "model-options-scroll",
            ui::ScrollMode::Top,
            body,
            t,
        ))
}

fn base_row(key: String, label: &str, highlighted: bool, t: &Theme) -> El {
    let mut item = row()
        .key(key)
        .label(label)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if highlighted {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover_group();
    if highlighted {
        item = item.focus_ring_for(FIELD, HAIRLINE, -HAIRLINE, t.accent);
    } else {
        item = item.hover(t.hover_wash);
    }
    item
}

fn level_row(
    column: &EffortColumn<'_>,
    level: &EffortLevel<'_>,
    highlighted: bool,
    t: &Theme,
) -> El {
    let mut item = base_row(
        format!("level-{}", level.value),
        &level.label,
        highlighted,
        t,
    )
    .role("radio")
    .checked(level.checked)
    .h(S32)
    .child(
        text(level.label.as_ref())
            .font(if level.checked { t.ui_medium } else { t.ui })
            .color(if level.checked { t.text_strong } else { t.text })
            .min_w(0.0)
            .shrink()
            .truncate()
            .a11y_hidden(),
    )
    .child(rune::spacer());
    if level.checked {
        item = item.child(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.text_strong)
                .a11y_hidden(),
        );
    } else if level.default {
        item = item.child(text("Default").font(t.micro).color(t.faint).a11y_hidden());
    }
    match (&column.model, column.option_id) {
        (Some(model), Some(option)) if column.editable => {
            item.on_press_tx(send(ModelPickerMsg::ChooseLevel {
                model: model.clone(),
                option: option.to_owned(),
                value: level.value.to_owned(),
            }))
        }
        _ => item.disabled(true),
    }
}

fn stack(label: &str, value: &str, t: &Theme) -> El {
    col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S2)
        .a11y_hidden()
        .child(
            text(sentence_label(label))
                .font(t.small)
                .color(t.muted)
                .truncate()
                .help_when_truncated(label),
        )
        .child(
            text(value)
                .font(t.ui)
                .color(t.text)
                .truncate()
                .help_when_truncated(value),
        )
}

fn extra_row(extra: ExtraOption<'_>, highlighted: bool, editable: bool, t: &Theme) -> El {
    let option = extra.option();
    let item = base_row(option_key(&option.id), &option.name, highlighted, t)
        .min_h(S40)
        .py(S6)
        .disabled(!editable);
    match extra {
        ExtraOption::Toggle { on, .. } => {
            let toggle = send(ModelPickerMsg::SetOption {
                option: option.id.clone(),
                value: viz_client::protocol::OptionValue::Boolean(!on),
            });
            item.role("switch")
                .checked(on)
                .on_press_tx(toggle.clone())
                .child(stack(&option.name, if on { "On" } else { "Off" }, t))
                .child(
                    rune_kit::Switch::new(on)
                        .on_toggle_tx(toggle)
                        .build(&t.k)
                        .a11y_hidden(),
                )
        }
        ExtraOption::Select { value, .. } => item
            .role("button")
            .a11y_value(value.map_or("", |choice| choice.name.as_str()))
            .on_press_tx(send(ModelPickerMsg::OpenSubmenu(option.id.clone())))
            .child(stack(
                &option.name,
                value.map_or("Choose…", |choice| choice.name.as_str()),
                t,
            ))
            .child(
                icon(&ui::CHEVRON, ICON_SMALL)
                    .color(t.faint)
                    .group_color(t.text)
                    .a11y_hidden(),
            ),
    }
}

/// The open select option's side submenu, anchored right of its row.
pub(super) fn submenu(s: &Desktop, column: &EffortColumn<'_>, t: &Theme) -> Option<El> {
    let state = &s.model.composer.model_menu;
    let open = state.submenu.as_deref()?;
    let (option, value) = column.extras.iter().find_map(|extra| match *extra {
        ExtraOption::Select { option, value } if option.id == open => Some((option, value)),
        ExtraOption::Select { .. } | ExtraOption::Toggle { .. } => None,
    })?;
    let mut items = col()
        .key("model-submenu-items")
        .role("listbox")
        .label(&option.name)
        .w_full()
        .padding(S4)
        .gap(S2)
        .instant_changes();
    for (index, choice) in option.choices.iter().enumerate() {
        let checked = value == Some(choice);
        let id = option.id.clone();
        let picked = choice.value.clone();
        let mut entry = ui::list_row(format!("model-submenu-{}", choice.value), &choice.name)
            .selected(checked)
            .highlighted(state.submenu_cursor == index)
            .focus_ring_for(FIELD)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::ModelPicker(ModelPickerMsg::SetOption {
                        option: id.clone(),
                        value: viz_client::protocol::OptionValue::Select(picked.clone()),
                    }),
                    tx,
                );
                s.dispatch(Msg::ModelPicker(ModelPickerMsg::CloseSubmenu), tx);
            });
        if let Some(description) = &choice.description {
            entry = entry.subtitle(description);
        }
        if checked {
            entry = entry.trailing(
                icon(&ui::CHECK, ICON_SMALL)
                    .color(t.text_strong)
                    .a11y_hidden(),
            );
        }
        items = items.child(entry.build(t).preserve_focus_on_press(true));
    }
    Some(
        ui::anchored_popover(
            "model-submenu",
            &option_key(&option.id),
            Placement::RightStart,
            SUBMENU_WIDTH,
            items,
            t,
        )
        .paint_order(41),
    )
}
