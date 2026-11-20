//! The model picker: one menu for the runner's model and its thinking level.
//! Everything shows as slugs, as the runner's ids: its trigger names both
//! (`claude-opus-5-5 · high`); the menu filters the models by a search field,
//! groups them by provider, and ends in the selected model's thinking levels.
//! Type to filter, arrows to move, Enter to choose.

use rune::{El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::{Placement, TextInput, popover, scrim};
use viz_client::protocol::{
    ConfigOption, DependentOption, Harness, OptionChoice, OptionKind, OptionValue,
};

use super::settings::{self, Options, Owner, Role, role};
use crate::{
    app::Desktop,
    backend::Command,
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_LIST_HEIGHT, MODEL_MENU_WIDTH, S4, S6, S8, S24, Size, Theme,
    },
    ui,
};

/// The picker's `open_picker` value and key.
const KEY: &str = "model";
const FIELD: &str = "model-query";
const LIST: &str = "model-list";

/// The open menu's search and keyboard highlight.
#[derive(Debug, Default)]
pub struct ModelMenu {
    query: TextEdit,
    /// The highlighted model's value; the current model, else the first
    /// match, when unset.
    active: Option<String>,
}

/// A session's or draft's model option and the thinking levels of the model
/// it has selected.
pub(super) struct Picker<'a> {
    target: Options<'a>,
    model: Option<&'a ConfigOption>,
    levels: Option<Levels<'a>>,
}

/// Thinking levels: the runner's own option for its current model, or the
/// levels it reported for another model a draft picked.
#[derive(Clone, Copy)]
enum Levels<'a> {
    Current(&'a ConfigOption),
    Model(&'a DependentOption),
}

impl<'a> Levels<'a> {
    fn id(self) -> &'a str {
        match self {
            Self::Current(option) => &option.id,
            Self::Model(option) => &option.id,
        }
    }

    fn name(self) -> &'a str {
        match self {
            Self::Current(option) => &option.name,
            Self::Model(option) => &option.name,
        }
    }

    fn choices(self) -> Vec<(&'a str, &'a str)> {
        match self {
            Self::Current(option) => option
                .choices
                .iter()
                .map(|choice| (choice.value.as_str(), choice.name.as_str()))
                .collect(),
            Self::Model(option) => option
                .choices
                .iter()
                .map(|choice| (choice.value.as_str(), choice.name.as_str()))
                .collect(),
        }
    }

    fn has(self, value: &str) -> bool {
        self.choices().iter().any(|(choice, _)| *choice == value)
    }

    /// The selected level: the draft's choice when this model offers it,
    /// else the runner's.
    fn value(self, target: Options<'a>) -> Option<&'a str> {
        let own = match self {
            Self::Current(option) => &option.value,
            Self::Model(option) => &option.value,
        };
        let chosen = match target {
            Options::Draft(chat) => chat.options.get(self.id()),
            Options::Conversation(_) => None,
        };
        [chosen, Some(own)]
            .into_iter()
            .flatten()
            .find_map(|value| match value {
                OptionValue::Select(value) if self.has(value) => Some(value.as_str()),
                _ => None,
            })
    }
}

impl<'a> Picker<'a> {
    /// The picker for `options`, when they have a model or thinking levels.
    pub(super) fn new(target: Options<'a>, options: &'a [ConfigOption]) -> Option<Self> {
        let find = |wanted: Role| {
            options
                .iter()
                .find(|option| role(option) == wanted && option.kind == OptionKind::Select)
        };
        let model = find(Role::Model);
        let thinking = find(Role::Thinking).map(Levels::Current);
        let levels = match (target, model) {
            (Options::Draft(_), Some(model)) => match target.value(model) {
                OptionValue::Select(value) if *target.value(model) != model.value => model
                    .choices
                    .iter()
                    .find(|choice| choice.value == *value)
                    .and_then(|choice| choice.thinking.as_ref())
                    .map(Levels::Model),
                _ => thinking,
            },
            _ => thinking,
        }
        // One level (a model without reasoning) is no choice.
        .filter(|levels| levels.choices().len() > 1);
        (model.is_some() || levels.is_some()).then_some(Self {
            target,
            model,
            levels,
        })
    }

    fn model_value(&self) -> Option<&'a str> {
        match self.model.map(|model| self.target.value(model)) {
            Some(OptionValue::Select(value)) => Some(value),
            _ => None,
        }
    }

    /// The trigger's label: the model's short name and the thinking level.
    fn label(&self) -> String {
        let model = self.model_value().map(|value| ui::slug(short(value)));
        let level = self
            .levels
            .and_then(|levels| levels.value(self.target))
            .map(str::to_lowercase);
        match (model, level, self.levels) {
            (Some(model), Some(level), _) => format!("{model} · {level}"),
            (Some(model), None, _) => model,
            (None, Some(level), Some(levels)) => format!("{} · {level}", ui::slug(levels.name())),
            _ => "model".to_owned(),
        }
    }
}

/// The trigger, and the menu while it's open, with the trigger's width.
pub(super) fn picker(
    s: &Desktop,
    picker: &Picker<'_>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> (El, f32) {
    let label = picker.label();
    let open = settings::is_open(s, KEY);
    let current = picker.model_value().map(row_key);
    let trigger = ui::select_trigger(KEY, "model", Some(&label), open, ui::MODEL_TRIGGER_MAX, t)
        .on_press_tx(move |s: &mut Desktop, tx| {
            if settings::is_open(s, KEY) {
                close(s);
                return;
            }
            s.open_picker = Some(KEY.to_owned());
            s.model_menu = ModelMenu::default();
            tx.focus_when_shown(FIELD, false);
            if let Some(current) = &current {
                tx.scroll_center_in_instant(LIST, current.clone());
            }
        });
    if open {
        menu(s, picker, t, layer);
    }
    (
        trigger,
        settings::trigger_width(&label, ui::MODEL_TRIGGER_MAX, t),
    )
}

/// Opens the picker in a preview scene, with `query` typed.
pub fn preview(s: &mut Desktop, query: &str) {
    s.open_picker = Some(KEY.to_owned());
    s.model_menu.query = TextEdit::new(query);
    s.model_menu.query.focused = true;
    s.model_menu.active = None;
    s.draft.focused = false;
}

fn close(s: &mut Desktop) {
    s.open_picker = None;
    s.model_menu = ModelMenu::default();
}

/// One model in the list.
struct Entry<'a> {
    value: &'a str,
    /// Its id without the provider, as a slug.
    label: String,
}

/// The models matching the search, grouped by provider in the runner's
/// order.
fn groups<'a>(model: &'a ConfigOption, query: &str) -> Vec<(Option<String>, Vec<Entry<'a>>)> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut groups: Vec<(Option<String>, Vec<Entry<'a>>)> = Vec::new();
    for choice in &model.choices {
        let group = group(choice);
        let haystack =
            format!("{} {}", group.as_deref().unwrap_or_default(), choice.value).to_lowercase();
        if !words.iter().all(|word| haystack.contains(word.as_str())) {
            continue;
        }
        let entry = Entry {
            value: &choice.value,
            label: ui::slug(short(&choice.value)),
        };
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((group, vec![entry])),
        }
    }
    groups
}

fn menu(s: &Desktop, picker: &Picker<'_>, t: &Theme, layer: &mut Vec<El>) {
    let k = &t.overlay;
    let mut body = col()
        .key("model-menu")
        .role("dialog")
        .label("model")
        .w(MODEL_MENU_WIDTH)
        .min_h(0.0)
        .radius(Size::Default.radius())
        .fill(k.c.layer_01)
        .focus_return(format!("{KEY}-trigger"))
        .modal(true, close);
    if let Some(model) = picker.model {
        let (field, list) = models(s, picker, model, t);
        body = body
            // Typing lands at once in the field; the list and the menu around it
            // spring to their new size (`on_change` below animates the commit).
            .child(row().w_full().px(S4).pt(S4).instant_changes().child(field))
            .child(ui::rule(t))
            .child(list);
    }
    if let Some(levels) = picker.levels {
        if picker.model.is_some() {
            body = body.child(ui::rule(t));
        }
        body = body.child(thinking(picker, levels, t));
    }
    layer.push(scrim("model-scrim", close));
    let body = k
        .shadow_layers(&k.elevation.floating)
        .fold(body, El::box_shadow);
    layer.push(popover::anchored(
        k,
        body,
        &format!("{KEY}-trigger"),
        Placement::TopStart,
        S6,
    ));
}

/// The search field and the matching models, grouped by provider.
fn models(s: &Desktop, picker: &Picker<'_>, model: &ConfigOption, t: &Theme) -> (El, El) {
    let current = picker.model_value();
    let groups = groups(model, &s.model_menu.query.text);
    let values: Vec<String> = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|entry| entry.value.to_owned()))
        .collect();
    let active = s
        .model_menu
        .active
        .as_deref()
        .filter(|active| values.iter().any(|value| value == active))
        .or_else(|| current.filter(|current| values.iter().any(|value| value == current)))
        .or_else(|| values.first().map(String::as_str))
        .map(str::to_owned);
    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label("models")
        .w_full()
        .max_h(MODEL_LIST_HEIGHT)
        .shrink()
        .min_h(0.0)
        .padding(S4)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    for (heading, entries) in &groups {
        if let Some(heading) = heading {
            list = list.child(
                row()
                    .key(format!("provider-{heading}"))
                    .h(S24)
                    .px(S8)
                    .center_y()
                    .child(ui::section(heading, t)),
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
            text("no models")
                .role("status")
                .font(t.small)
                .color(t.faint)
                .px(S8)
                .py(S8),
        );
    }
    let owner = picker.target.owner();
    let id = model.id.clone();
    let field = TextInput::new(&s.model_menu.query)
        .label("search models")
        .placeholder("search models")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Default.height())
        .active_descendant(active.as_deref().map(row_key))
        .on_edit(|s: &mut Desktop| &mut s.model_menu.query)
        .on_change(|s: &mut Desktop, tx| {
            s.model_menu.active = None;
            tx.scroll_to_instant(LIST, 0.0, 0.0);
            // Typing commits instantly by default; the filtered list should
            // grow and shrink smoothly, its dropped rows fading out.
            tx.animated();
        })
        .on_key(move |s, ev, tx| on_key(s, ev, tx, &values, active.as_deref(), owner, &id))
        .build(&t.overlay)
        .key(FIELD);
    (field, list)
}

fn row_key(value: &str) -> String {
    format!("model-{value}")
}

fn model_row(picker: &Picker<'_>, entry: &Entry<'_>, active: bool, current: bool, t: &Theme) -> El {
    let owner = picker.target.owner();
    let id = picker
        .model
        .map_or_else(String::new, |model| model.id.clone());
    let value = entry.value.to_owned();
    let mut item = row()
        .key(row_key(entry.value))
        .role("option")
        .label(&entry.label)
        .checked(current)
        .focus_selected(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .h(Size::Default.height())
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .hover(t.hover)
        .on_press_tx(move |s: &mut Desktop, tx| choose_model(s, tx, owner, &id, &value))
        .child(
            text(&entry.label)
                .font(t.ui)
                .color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(rune::spacer())
        .child(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.muted)
                .opacity(if current { 1.0 } else { 0.0 })
                .a11y_hidden(),
        );
    if active {
        item = item.fill(t.selected);
    }
    item
}

/// The selected model's thinking levels as a row of choices.
fn thinking(picker: &Picker<'_>, levels: Levels<'_>, t: &Theme) -> El {
    let selected = levels.value(picker.target);
    let owner = picker.target.owner();
    let chips = levels.choices().into_iter().map(|(value, _)| {
        let on = selected == Some(value);
        let label = value.to_lowercase();
        let id = levels.id().to_owned();
        let choice = OptionValue::Select(value.to_owned());
        row()
            .key(format!("level-{value}"))
            .role("radio")
            .label(&label)
            .checked(on)
            .h(Size::Small.height())
            .px(S8)
            .center()
            .radius(Size::Small.radius())
            .fill(if on {
                t.selected
            } else {
                rune::Color::TRANSPARENT
            })
            .hover(if on { t.selected } else { t.hover })
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx| {
                close(s);
                tx.focus("draft", false);
                if !on {
                    settings::apply(s, tx, owner, id.clone(), choice.clone());
                }
            })
            .child(
                text(label)
                    .font(if on { t.ui_medium } else { t.ui })
                    .color(if on { t.text } else { t.muted })
                    .a11y_hidden(),
            )
    });
    let name = levels.name().to_lowercase();
    col()
        .key("model-thinking")
        .w_full()
        .px(S8)
        .pt(S6)
        .pb(S8)
        .gap(S4)
        .child(ui::section(&name, t))
        .child(
            row()
                .key("levels")
                .role("radiogroup")
                .label(name)
                .w_full()
                .gap(S4)
                .flex_wrap()
                .children(chips),
        )
}

fn on_key(
    s: &mut Desktop,
    ev: &KeyEvent,
    tx: &mut Tx,
    values: &[String],
    active: Option<&str>,
    owner: Owner,
    id: &str,
) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.model_menu.query.preedit().is_some() || !matches!(ev.key, Key::Down | Key::Up | Key::Enter)
    {
        return false;
    }
    if values.is_empty() {
        return true;
    }
    let index = active
        .and_then(|active| values.iter().position(|value| value == active))
        .unwrap_or_default();
    if ev.key == Key::Enter {
        if !tx.key_repeat() {
            choose_model(s, tx, owner, id, &values[index]);
        }
    } else {
        let next = if ev.key == Key::Down {
            (index + 1) % values.len()
        } else {
            index.checked_sub(1).unwrap_or(values.len() - 1)
        };
        s.model_menu.active = Some(values[next].clone());
        tx.scroll_into_view_in(LIST, row_key(&values[next]));
    }
    true
}

/// Selects model `value`. A session applies it and reports the new model's
/// levels itself. A draft keeps its thinking level only where the new model
/// offers it, and asks the runner for a model's levels it hasn't reported.
fn choose_model(s: &mut Desktop, tx: &mut Tx, owner: Owner, id: &str, value: &str) {
    close(s);
    tx.focus("draft", false);
    let chosen = OptionValue::Select(value.to_owned());
    let draft = match owner {
        Owner::Conversation(_) => {
            let changed = s.conversation.as_ref().is_some_and(|state| {
                state
                    .options
                    .iter()
                    .any(|option| option.id == id && option.value != chosen)
            });
            if changed {
                settings::apply(s, tx, owner, id.to_owned(), chosen);
            }
            return;
        }
        Owner::Draft(draft) => draft,
    };
    let Some(chat) = s.new_chat.as_mut().filter(|chat| chat.id == draft) else {
        return;
    };
    chat.options.insert(id.to_owned(), chosen);
    let Some(harness) = chat.active_harness(&s.harnesses) else {
        return;
    };
    // The draft's level under the new model: kept where it offers it, else
    // the model's default. Until the runner reports an unseen model's
    // levels, its default applies.
    let (stale, fetch) = match Picker::new(Options::Draft(chat), &harness.options) {
        Some(Picker {
            levels: Some(levels),
            ..
        }) => {
            let kept = matches!(
                chat.options.get(levels.id()),
                Some(OptionValue::Select(level)) if levels.has(level)
            );
            ((!kept).then(|| vec![levels.id().to_owned()]), None)
        }
        _ if unseen(harness, id, value) => {
            (Some(thinking_ids(harness)), Some(harness.name.clone()))
        }
        _ => (None, None),
    };
    for level in stale.into_iter().flatten() {
        chat.options.remove(&level);
    }
    if let Some(harness) = fetch {
        s.send(
            Command::HarnessOptions {
                harness,
                model: value.to_owned(),
            },
            tx,
        );
    }
}

/// Whether the runner has yet to report model `value`'s thinking levels.
fn unseen(harness: &Harness, id: &str, value: &str) -> bool {
    harness
        .options
        .iter()
        .find(|option| option.id == id)
        .and_then(|model| model.choices.iter().find(|choice| choice.value == value))
        .is_some_and(|choice| choice.value != value_of(harness, id) && choice.thinking.is_none())
}

fn value_of<'a>(harness: &'a Harness, id: &str) -> &'a str {
    harness
        .options
        .iter()
        .find(|option| option.id == id)
        .and_then(|option| match &option.value {
            OptionValue::Select(value) => Some(value.as_str()),
            OptionValue::Boolean(_) => None,
        })
        .unwrap_or_default()
}

/// The ids a draft's thinking level may be stored under for `harness`.
fn thinking_ids(harness: &Harness) -> Vec<String> {
    harness
        .options
        .iter()
        .flat_map(|option| match role(option) {
            Role::Thinking => vec![option.id.clone()],
            Role::Model => option
                .choices
                .iter()
                .filter_map(|choice| choice.thinking.as_ref().map(|levels| levels.id.clone()))
                .collect(),
            Role::Mode | Role::Other => Vec::new(),
        })
        .collect()
}

/// A model's provider as a slug: the runner's group (`OpenCode Go` is
/// `opencode-go`), else the provider before a `/` in its id.
fn group(choice: &OptionChoice) -> Option<String> {
    choice
        .group
        .as_deref()
        .or_else(|| choice.value.split_once('/').map(|(provider, _)| provider))
        .map(ui::slug)
}

/// A model id without its provider.
fn short(value: &str) -> &str {
    value.split_once('/').map_or(value, |(_, model)| model)
}
