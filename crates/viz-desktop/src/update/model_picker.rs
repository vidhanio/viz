//! Pure transitions for the unified model picker: opening and closing, rail
//! tabs, search, choosing models across harnesses, effort levels, extra ACP
//! options and their submenus, speed, favorites, access mode, and full
//! keyboard navigation over a focus that stays in the search field.

use viz_client::protocol::OptionValue;

use super::{
    model_picker_keys::{PickerKey, on_key},
    selection::focus_draft,
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect},
    model::{
        ACCESS_PICKER, ExtraNav, MODEL_PICKER, Model, ModelMenuState, ModelPicker, ModelRef,
        OptionRole, OptionTarget, PickerColumn, RailTab, access_view, option_role, picker_nav,
        picker_scope, speed, thinking_ids, unseen_model_levels,
    },
};

/// Element key of the model list's scroll view.
pub const MODEL_LIST: &str = "model-list";
/// Element key of the picker's search field.
pub const MODEL_FIELD: &str = "model-query";

/// Messages of the model picker and access-mode popovers.
#[derive(Debug, Clone)]
pub enum ModelPickerMsg {
    /// Open or close the picker (trigger, `Cmd/Ctrl+/`).
    Toggle,
    /// Close the picker and return focus to the composer.
    Close,
    /// Search text changed.
    Query(String),
    /// Switch the rail tab.
    Tab(RailTab),
    /// Pointer previewed a model.
    Highlight(ModelRef),
    /// Choose a model.
    Choose(ModelRef),
    /// Star or unstar a model.
    ToggleFavorite(ModelRef),
    /// Expand or collapse a folded group.
    ToggleFold(String),
    /// Choose an effort level for a model (choosing the model too).
    ChooseLevel {
        /// The model configured.
        model: ModelRef,
        /// Thinking option id.
        option: String,
        /// Level value.
        value: String,
    },
    /// Set an extra option of the scope.
    SetOption {
        /// Option id.
        option: String,
        /// New value.
        value: OptionValue,
    },
    /// Open the side submenu of a select option.
    OpenSubmenu(String),
    /// Close the side submenu.
    CloseSubmenu,
    /// Switch the speed tier.
    SetFast(bool),
    /// A navigation key from the search field.
    Key(PickerKey),
    /// Open or close the access popover (footer, or standalone on Home).
    ToggleAccess,
    /// Close the access popover.
    CloseAccess,
    /// Preview an access mode's description.
    PreviewAccess(Option<usize>),
    /// Choose an access mode by index.
    ChooseAccess(usize),
}

/// Applies a picker message.
pub(super) fn update(model: &mut Model, msg: ModelPickerMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    let fx = &mut effects;
    match msg {
        ModelPickerMsg::Toggle => {
            if model.composer.open_picker.as_deref() == Some(MODEL_PICKER) {
                close(model, fx);
            } else {
                open(model, fx);
            }
        }
        ModelPickerMsg::Close => close(model, fx),
        ModelPickerMsg::Query(query) => {
            let menu = &mut model.composer.model_menu;
            menu.query = query;
            menu.highlight = None;
            menu.column = PickerColumn::Models;
            menu.submenu = None;
            scroll_top(fx);
        }
        ModelPickerMsg::Tab(tab) => switch_tab(model, tab, fx),
        ModelPickerMsg::Highlight(row) => model.composer.model_menu.highlight = Some(row),
        ModelPickerMsg::Choose(row) => choose_and_advance(model, &row, fx),
        ModelPickerMsg::ToggleFavorite(row) => {
            let key = row.favorite_key();
            let favorites = &mut model.composer.favorite_models;
            if let Some(index) = favorites.iter().position(|f| *f == key) {
                favorites.remove(index);
            } else {
                favorites.push(key);
            }
            fx.push(Effect::SavePrefs(model.to_prefs()));
        }
        ModelPickerMsg::ToggleFold(key) => {
            let expanded = &mut model.composer.model_menu.expanded;
            if !expanded.remove(&key) {
                expanded.insert(key);
            }
        }
        ModelPickerMsg::ChooseLevel {
            model: row,
            option,
            value,
        } => choose_level(model, &row, option, value, fx),
        ModelPickerMsg::SetOption { option, value } => apply_option(model, option, value, fx),
        ModelPickerMsg::OpenSubmenu(option) => open_submenu(model, option),
        ModelPickerMsg::CloseSubmenu => model.composer.model_menu.submenu = None,
        ModelPickerMsg::SetFast(fast) => {
            let change = picker_scope(model).and_then(|scope| {
                let speed = speed(&scope)?;
                Some((
                    speed.option.id.clone(),
                    if fast { speed.fast } else { speed.standard },
                ))
            });
            if let Some((option, value)) = change {
                apply_option(model, option, value, fx);
            }
        }
        ModelPickerMsg::Key(key) => on_key(model, key, fx),
        ModelPickerMsg::ToggleAccess => {
            if model.composer.open_picker.as_deref() == Some(MODEL_PICKER) {
                let menu = &mut model.composer.model_menu;
                menu.access_open = !menu.access_open;
                menu.access_preview = None;
            } else if model.composer.open_picker.as_deref() == Some(ACCESS_PICKER) {
                model.composer.open_picker = None;
            } else {
                model.composer.open_picker = Some(ACCESS_PICKER.to_owned());
                model.composer.model_menu.access_preview = None;
            }
        }
        ModelPickerMsg::CloseAccess => close_access(model),
        ModelPickerMsg::PreviewAccess(index) => model.composer.model_menu.access_preview = index,
        ModelPickerMsg::ChooseAccess(index) => {
            choose_access(model, index, fx);
            close_access(model);
        }
    }
    effects
}

fn open(model: &mut Model, fx: &mut Vec<Effect>) {
    model.composer.open_picker = Some(MODEL_PICKER.to_owned());
    model.composer.model_menu = ModelMenuState::default();
    let selected = picker_scope(model).and_then(|scope| scope.selected());
    fx.push(Effect::SyncEdit(EditSync::Clear(EditTarget::ModelQuery)));
    fx.push(Effect::Focus {
        key: MODEL_FIELD,
        select_all: false,
        when_shown: true,
    });
    if let Some(selected) = selected {
        fx.push(Effect::ScrollCenterInstant {
            list: MODEL_LIST,
            item: selected.row_key(),
        });
    }
    fx.push(Effect::Server(ServerEffect::Harnesses));
}

/// Closes the model picker and returns focus to the composer.
pub(super) fn close(model: &mut Model, fx: &mut Vec<Effect>) {
    model.composer.open_picker = None;
    model.composer.model_menu = ModelMenuState::default();
    fx.push(Effect::SyncEdit(EditSync::Clear(EditTarget::ModelQuery)));
    fx.push(focus_draft());
}

pub(super) fn close_access(model: &mut Model) {
    if model.composer.open_picker.as_deref() == Some(ACCESS_PICKER) {
        model.composer.open_picker = None;
    }
    model.composer.model_menu.access_open = false;
    model.composer.model_menu.access_preview = None;
}

pub(super) fn scroll_top(fx: &mut Vec<Effect>) {
    fx.push(Effect::ScrollToInstant {
        key: MODEL_LIST,
        x: 0.0,
        y: 0.0,
    });
}

pub(super) fn switch_tab(model: &mut Model, tab: RailTab, fx: &mut Vec<Effect>) {
    let menu = &mut model.composer.model_menu;
    if menu.tab.as_ref() != Some(&tab) {
        menu.tab = Some(tab);
        menu.highlight = None;
        menu.submenu = None;
        scroll_top(fx);
    }
}

pub(super) fn open_submenu(model: &mut Model, option: String) {
    let nav = picker_nav(model);
    let menu = &mut model.composer.model_menu;
    let found = nav.as_ref().and_then(|nav| {
        nav.extras
            .iter()
            .enumerate()
            .find_map(|(index, extra)| match extra {
                ExtraNav::Select {
                    option: id,
                    current,
                    ..
                } if *id == option => Some((nav.levels.len() + index, *current)),
                ExtraNav::Select { .. } | ExtraNav::Toggle { .. } => None,
            })
    });
    if let Some((row, current)) = found {
        menu.column = PickerColumn::Options;
        menu.option_cursor = row;
        menu.submenu = Some(option);
        menu.submenu_cursor = current;
    }
}

/// Chooses `row`, then moves the keyboard to its effort levels, or closes the
/// picker when it has none.
pub(super) fn choose_and_advance(model: &mut Model, row: &ModelRef, fx: &mut Vec<Effect>) {
    if !choose_model(model, row, fx) {
        return;
    }
    let nav = picker_nav(model);
    let menu = &mut model.composer.model_menu;
    menu.highlight = Some(row.clone());
    match nav {
        Some(nav) if !nav.levels.is_empty() || menu.loading_levels.is_some() => {
            menu.column = PickerColumn::Options;
            menu.option_cursor = nav.checked_level.unwrap_or_default();
        }
        _ => close(model, fx),
    }
}

/// Selects `row` for the scope. Drafts may switch harness; threads keep
/// theirs. Returns whether the model was accepted.
fn choose_model(model: &mut Model, row: &ModelRef, fx: &mut Vec<Effect>) -> bool {
    let Some(scope) = picker_scope(model) else {
        return false;
    };
    let option_id = model
        .harnesses()
        .iter()
        .find(|harness| harness.id == row.harness)
        .map(|harness| harness.options.as_slice())
        .into_iter()
        .chain((scope.harness == row.harness).then_some(scope.options))
        .flatten()
        .find(|option| option_role(option) == OptionRole::Model)
        .map(|option| option.id.clone());
    let Some(id) = option_id else {
        return false;
    };
    let chosen = OptionValue::Select(row.value.clone());
    if let OptionTarget::Thread(thread) = scope.target {
        if scope.harness != row.harness {
            return false;
        }
        let changed = scope
            .options
            .iter()
            .any(|option| option.id == id && option.value != chosen);
        if changed {
            model.composer.model_menu.loading_levels = Some(row.value.clone());
            fx.push(Effect::Server(ServerEffect::Configure {
                thread,
                option: id,
                value: chosen,
            }));
        }
        return true;
    }
    let harnesses = model.harnesses().to_vec();
    let Some(draft) = model.selection.new_thread.as_mut() else {
        return false;
    };
    if draft
        .active_harness(&harnesses)
        .is_none_or(|h| h.id != row.harness)
    {
        draft.harness = Some(row.harness.clone());
        draft.options.clear();
        draft.access = None;
    }
    draft.options.insert(id.clone(), chosen);
    let Some(harness) = draft.active_harness(&harnesses) else {
        return true;
    };
    let (stale, fetch) = match ModelPicker::new(OptionTarget::Draft(draft), &harness.options) {
        Some(ModelPicker {
            levels: Some(levels),
            ..
        }) => {
            let kept = matches!(
                draft.options.get(levels.id()),
                Some(OptionValue::Select(level)) if levels.has(level)
            );
            ((!kept).then(|| vec![levels.id().to_owned()]), false)
        }
        _ if unseen_model_levels(harness, &id, &row.value) => (Some(thinking_ids(harness)), true),
        _ => (None, false),
    };
    for level in stale.into_iter().flatten() {
        draft.options.remove(&level);
    }
    if fetch {
        model.composer.model_menu.loading_levels = Some(row.value.clone());
        fx.push(Effect::Server(ServerEffect::HarnessOptions {
            harness: row.harness.clone(),
            model: row.value.clone(),
        }));
    } else {
        model.composer.model_menu.loading_levels = None;
    }
    true
}

pub(super) fn choose_level(
    model: &mut Model,
    row: &ModelRef,
    option: String,
    value: String,
    fx: &mut Vec<Effect>,
) {
    let selected = picker_scope(model).and_then(|scope| scope.selected());
    if selected.as_ref() != Some(row) && !choose_model(model, row, fx) {
        return;
    }
    apply_option(model, option, OptionValue::Select(value), fx);
    close(model, fx);
}

/// Applies an option to the scope: `thread.configure`, or the draft's launch
/// options.
pub(super) fn apply_option(
    model: &mut Model,
    option: String,
    value: OptionValue,
    fx: &mut Vec<Effect>,
) {
    if let Some(draft) = model.selection.new_thread.as_mut() {
        draft.options.insert(option, value);
    } else if let Some(thread) = model.selection.selected {
        fx.push(Effect::Server(ServerEffect::Configure {
            thread,
            option,
            value,
        }));
    }
}

fn choose_access(model: &mut Model, index: usize, fx: &mut Vec<Effect>) {
    let Some(view) = access_view(model) else {
        return;
    };
    let Some(mode) = view.modes.get(index).cloned() else {
        return;
    };
    if let Some(thread) = view.thread {
        if mode != *view.current {
            fx.push(Effect::Server(ServerEffect::Access {
                thread,
                access: Some(mode),
            }));
        }
        return;
    }
    let natives: Vec<_> = view
        .modes
        .iter()
        .filter_map(|mode| mode.native.as_ref().map(|native| native.option.clone()))
        .collect();
    if let Some(draft) = model.selection.new_thread.as_mut() {
        for option in natives {
            draft.options.remove(&option);
        }
        if let Some(native) = &mode.native {
            draft.options.insert(
                native.option.clone(),
                OptionValue::Select(native.value.clone()),
            );
        }
        draft.access = Some(mode);
    }
}
