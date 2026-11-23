//! Keyboard navigation for the model picker. Focus stays in the search
//! field; [`PickerColumn`] routes each key to the rail, the model list, the
//! options column or the open side submenu.

use viz_client::protocol::OptionValue;

use super::model_picker::{
    MODEL_LIST, apply_option, choose_and_advance, choose_level, close, open_submenu, switch_tab,
};
use crate::{
    effects::Effect,
    model::{ExtraNav, Model, PickerColumn, PickerNav, picker_nav},
};

/// A key the picker's search field forwards, translated at the view edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKey {
    /// Previous row.
    Up,
    /// Next row.
    Down,
    /// Left; `edge` when the field is empty or its caret is at the start.
    Left {
        /// Whether the caret cannot move further left.
        edge: bool,
    },
    /// Right; `edge` when the caret is at the end of the text.
    Right {
        /// Whether the caret cannot move further right.
        edge: bool,
    },
    /// Confirm.
    Enter,
    /// Toggle or confirm outside the search column.
    Space,
    /// Next column; previous with Shift.
    Tab {
        /// Shift held.
        back: bool,
    },
    /// Close the submenu, else the picker.
    Escape,
    /// Jump to the n-th row (1-based).
    Digit(u8),
    /// Type-ahead in the options column.
    Char(char),
}

/// Routes a picker key.
pub(super) fn on_key(model: &mut Model, key: PickerKey, fx: &mut Vec<Effect>) {
    let Some(nav) = picker_nav(model) else {
        return;
    };
    if model.composer.model_menu.submenu.is_some() {
        submenu_key(model, &nav, key, fx);
        return;
    }
    let menu = &mut model.composer.model_menu;
    let rows = nav.rows.len();
    let index = menu
        .highlight
        .as_ref()
        .or(nav.preview.as_ref())
        .and_then(|wanted| nav.rows.iter().position(|(row, _)| row == wanted));
    let options = nav.option_rows();
    match (menu.column, key) {
        (_, PickerKey::Escape) => close(model, fx),
        (_, PickerKey::Tab { back }) => {
            let order: &[PickerColumn] = if nav.searching {
                &[PickerColumn::Models, PickerColumn::Options]
            } else {
                &[
                    PickerColumn::Rail,
                    PickerColumn::Models,
                    PickerColumn::Options,
                ]
            };
            let at = order.iter().position(|c| *c == menu.column).unwrap_or(0);
            let next = if back { at + order.len() - 1 } else { at + 1 } % order.len();
            menu.column = order[next];
            menu.option_cursor = nav
                .checked_level
                .unwrap_or_default()
                .min(options.saturating_sub(1));
        }
        (_, PickerKey::Digit(n)) => {
            if let Some((row, false)) = nav.rows.get(usize::from(n.saturating_sub(1))) {
                let row = row.clone();
                choose_and_advance(model, &row, fx);
            }
        }
        (PickerColumn::Rail, PickerKey::Up | PickerKey::Down) => {
            let at = nav.tabs.iter().position(|tab| *tab == nav.tab).unwrap_or(0);
            let len = nav.tabs.len();
            let next = if key == PickerKey::Up {
                at + len - 1
            } else {
                at + 1
            } % len;
            switch_tab(model, nav.tabs[next].clone(), fx);
        }
        (PickerColumn::Rail, PickerKey::Right { .. } | PickerKey::Enter) => {
            menu.column = PickerColumn::Models;
        }
        (PickerColumn::Models, PickerKey::Up | PickerKey::Down) if rows > 0 => {
            let at = index.unwrap_or(0);
            let next = if key == PickerKey::Up {
                at + rows - 1
            } else {
                at + 1
            } % rows;
            let row = nav.rows[next].0.clone();
            fx.push(Effect::ScrollIntoView {
                list: MODEL_LIST,
                item: row.row_key(),
            });
            menu.highlight = Some(row);
        }
        (PickerColumn::Models, PickerKey::Enter | PickerKey::Right { edge: true }) => {
            if let Some((row, false)) = index.and_then(|i| nav.rows.get(i)) {
                let row = row.clone();
                choose_and_advance(model, &row, fx);
            }
        }
        (PickerColumn::Models, PickerKey::Left { edge: true }) if !nav.searching => {
            menu.column = PickerColumn::Rail;
        }
        (PickerColumn::Options, _) => options_key(model, &nav, key, fx),
        _ => {}
    }
}

fn options_key(model: &mut Model, nav: &PickerNav, key: PickerKey, fx: &mut Vec<Effect>) {
    let menu = &mut model.composer.model_menu;
    let options = nav.option_rows();
    match key {
        PickerKey::Up | PickerKey::Down if options > 0 => {
            let at = menu.option_cursor.min(options - 1);
            menu.option_cursor = if key == PickerKey::Up {
                at + options - 1
            } else {
                at + 1
            } % options;
        }
        PickerKey::Left { .. } => menu.column = PickerColumn::Models,
        PickerKey::Char(ch) => {
            if let Some(at) = nav.levels.iter().position(|(_, _, label)| {
                label
                    .chars()
                    .next()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&ch))
            }) {
                menu.option_cursor = at;
            }
        }
        PickerKey::Enter | PickerKey::Space | PickerKey::Right { .. } => {
            options_activate(model, nav, key, fx);
        }
        _ => {}
    }
}

fn options_activate(model: &mut Model, nav: &PickerNav, key: PickerKey, fx: &mut Vec<Effect>) {
    if !nav.editable {
        return;
    }
    let cursor = model.composer.model_menu.option_cursor;
    if let Some((option, value, _)) = nav.levels.get(cursor) {
        if matches!(key, PickerKey::Enter | PickerKey::Space)
            && let Some(row) = nav.preview.clone()
        {
            choose_level(model, &row, option.clone(), value.clone(), fx);
        }
        return;
    }
    match nav.extras.get(cursor - nav.levels.len()) {
        Some(ExtraNav::Toggle { option, on }) if key != (PickerKey::Right { edge: true }) => {
            apply_option(model, option.clone(), OptionValue::Boolean(!on), fx);
        }
        Some(ExtraNav::Select { option, .. }) if key != PickerKey::Space => {
            open_submenu(model, option.clone());
        }
        _ => {}
    }
}

fn submenu_key(model: &mut Model, nav: &PickerNav, key: PickerKey, fx: &mut Vec<Effect>) {
    let menu = &mut model.composer.model_menu;
    let Some(ExtraNav::Select {
        option, choices, ..
    }) = nav.extras.iter().find(|extra| {
        matches!(extra, ExtraNav::Select { option, .. } if menu.submenu.as_ref() == Some(option))
    })
    else {
        menu.submenu = None;
        return;
    };
    let len = choices.len().max(1);
    match key {
        PickerKey::Left { .. } | PickerKey::Escape => menu.submenu = None,
        PickerKey::Up => menu.submenu_cursor = (menu.submenu_cursor + len - 1) % len,
        PickerKey::Down => menu.submenu_cursor = (menu.submenu_cursor + 1) % len,
        PickerKey::Enter | PickerKey::Space => {
            if let Some(value) = choices.get(menu.submenu_cursor) {
                let (option, value) = (option.clone(), OptionValue::Select(value.clone()));
                menu.submenu = None;
                apply_option(model, option, value, fx);
            }
        }
        _ => {}
    }
}
