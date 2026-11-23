//! Pure transitions for the model-and-thinking picker menu and runner options.

use viz_client::protocol::OptionValue;

use super::{NavKey, selection::focus_draft};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect},
    model::{
        Model, ModelMenuState, ModelPicker, OptionOwner, OptionTarget, thinking_ids,
        unseen_model_levels,
    },
};

pub(super) const MODEL_LIST: &str = "model-list";

pub(super) fn close_model_menu(model: &mut Model, effects: &mut Vec<Effect>) {
    model.composer.open_picker = None;
    model.composer.model_menu = ModelMenuState::default();
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::ModelQuery)));
}

#[expect(
    clippy::too_many_arguments,
    reason = "keyboard navigation needs the filtered list, highlight, committed value, and owner"
)]
pub(super) fn handle_model_key(
    model: &mut Model,
    key: NavKey,
    values: &[String],
    active: Option<&str>,
    committed: Option<&str>,
    owner: OptionOwner,
    option_id: &str,
    effects: &mut Vec<Effect>,
) {
    if values.is_empty() {
        return;
    }
    let index = active
        .and_then(|active| values.iter().position(|value| value == active))
        .unwrap_or_default();
    match key {
        NavKey::Enter => {
            let chosen = &values[index];
            if committed == Some(chosen.as_str()) {
                close_model_menu(model, effects);
                effects.push(focus_draft());
            } else {
                choose_model(model, owner, option_id, chosen, effects);
            }
        }
        NavKey::Down | NavKey::Up => {
            let next = if key == NavKey::Down {
                (index + 1) % values.len()
            } else {
                index.checked_sub(1).unwrap_or(values.len() - 1)
            };
            let next_value = values[next].clone();
            model.composer.model_menu.active = Some(next_value.clone());
            effects.push(Effect::ScrollIntoView {
                list: MODEL_LIST,
                item: format!("model-{next_value}"),
            });
        }
        NavKey::Escape | NavKey::Tab => {}
    }
}

pub(super) fn choose_model(
    model: &mut Model,
    owner: OptionOwner,
    id: &str,
    value: &str,
    effects: &mut Vec<Effect>,
) {
    model.composer.model_menu.active = Some(value.to_owned());
    model.composer.model_menu.chosen = None;
    let chosen = OptionValue::Select(value.to_owned());
    let draft = match owner {
        OptionOwner::Thread(_) => {
            let changed = model.thread_state().is_some_and(|state| {
                state
                    .options
                    .iter()
                    .any(|option| option.id == id && option.value != chosen)
            });
            if changed {
                model.composer.model_menu.loading_levels = Some(value.to_owned());
                apply_option(model, owner, id.to_owned(), chosen, effects);
            }
            return;
        }
        OptionOwner::Draft(draft) => draft,
    };
    let harnesses = model.harnesses().to_vec();
    let Some(thread) = model
        .selection
        .new_thread
        .as_mut()
        .filter(|thread| thread.id == draft)
    else {
        return;
    };
    thread.options.insert(id.to_owned(), chosen);
    let Some(harness) = thread.active_harness(&harnesses) else {
        return;
    };
    let (stale, fetch) = match ModelPicker::new(OptionTarget::Draft(thread), &harness.options) {
        Some(ModelPicker {
            levels: Some(levels),
            ..
        }) => {
            let kept = matches!(
                thread.options.get(levels.id()),
                Some(OptionValue::Select(level)) if levels.has(level)
            );
            ((!kept).then(|| vec![levels.id().to_owned()]), None)
        }
        _ if unseen_model_levels(harness, id, value) => {
            (Some(thinking_ids(harness)), Some(harness.id.clone()))
        }
        _ => (None, None),
    };
    for level in stale.into_iter().flatten() {
        thread.options.remove(&level);
    }
    if let Some(harness) = fetch {
        model.composer.model_menu.loading_levels = Some(value.to_owned());
        effects.push(Effect::Server(ServerEffect::HarnessOptions {
            harness,
            model: value.to_owned(),
        }));
    } else {
        model.composer.model_menu.loading_levels = None;
    }
}

pub(super) fn apply_option(
    model: &mut Model,
    owner: OptionOwner,
    option: String,
    value: OptionValue,
    effects: &mut Vec<Effect>,
) {
    match owner {
        OptionOwner::Thread(thread) => {
            effects.push(Effect::Server(ServerEffect::Configure {
                thread,
                option,
                value,
            }));
        }
        OptionOwner::Draft(draft) => {
            if let Some(thread) = model
                .selection
                .new_thread
                .as_mut()
                .filter(|thread| thread.id == draft)
            {
                thread.options.insert(option, value);
            }
        }
    }
}
