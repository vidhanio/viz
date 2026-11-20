//! The model picker: one menu for the runner's model and its thinking level.
//! The trigger names both. The menu groups models by provider and ends with
//! the selected model's thinking levels. Search matches both display names
//! and IDs; arrows move the highlight, and Enter chooses the model.

use std::{borrow::Cow, collections::HashMap};

use rune::{
    Align, Effect, El, Key, KeyEvent, MotionIntent, ScrollSpec, Side, Spring, SpringPair, TextEdit,
    Transition, Tx, col, icon, row, text,
};
use rune_kit::{TextInput, scrim};
use viz_client::protocol::{
    ConfigOption, DependentOption, Harness, OptionChoice, OptionKind, OptionValue,
};

use super::settings::{self, Options, Owner, Role, role};
use crate::{
    app::Desktop,
    backend::Command,
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_LIST_HEIGHT, MODEL_MENU_WIDTH, S4, S6, S8, S16, S24, Size,
        Theme,
    },
    ui,
};

/// The picker's `open_picker` value and key.
const KEY: &str = "model";
const FIELD: &str = "model-query";
const LIST: &str = "model-list";

/// The open menu's search, keyboard highlight and pending thinking choice.
#[derive(Debug, Default)]
pub struct ModelMenu {
    query: TextEdit,
    /// The highlighted model's value; the current model, else the first
    /// match, when unset.
    active: Option<String>,
    /// The thinking level last chosen here (option id, value), shown until
    /// the menu closes or the model changes: a session's runner reports a
    /// level back only once it has applied it.
    chosen: Option<(String, String)>,
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

    fn choices(self) -> impl ExactSizeIterator<Item = (&'a str, &'a str)> {
        let len = match self {
            Self::Current(option) => option.choices.len(),
            Self::Model(option) => option.choices.len(),
        };
        (0..len).map(move |index| match self {
            Self::Current(option) => {
                let choice = &option.choices[index];
                (choice.value.as_str(), choice.name.as_str())
            }
            Self::Model(option) => {
                let choice = &option.choices[index];
                (choice.value.as_str(), choice.name.as_str())
            }
        })
    }

    fn has(self, value: &str) -> bool {
        self.choices().any(|(choice, _)| choice == value)
    }

    fn label(self, value: &'a str) -> Cow<'a, str> {
        self.choices()
            .find(|(choice, _)| *choice == value)
            .map_or_else(
                || ui::thinking_label(value),
                |(_, name)| self.choice_label(value, name),
            )
    }

    fn choice_label(self, value: &'a str, name: &'a str) -> Cow<'a, str> {
        let name = name
            .split_once(':')
            .filter(|(prefix, _)| prefix.trim().eq_ignore_ascii_case(self.name()))
            .map_or(name, |(_, rest)| rest)
            .trim();
        ui::thinking_label(if name.is_empty() { value } else { name })
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

    /// The trigger shows the model and thinking level's display names.
    fn label(&self) -> Cow<'a, str> {
        let model = self.model.zip(self.model_value()).map(|(model, value)| {
            model
                .choices
                .iter()
                .find(|choice| choice.value == value)
                .map_or(value, model_name)
        });
        let level = self.levels.and_then(|levels| {
            levels
                .value(self.target)
                .map(|value| (levels, levels.label(value)))
        });
        match (model, level) {
            (Some(model), Some((_, level))) => Cow::Owned(format!("{model} · {level}")),
            (Some(model), None) => Cow::Borrowed(model),
            (None, Some((levels, level))) => {
                Cow::Owned(format!("{} · {level}", ui::sentence_label(levels.name())))
            }
            (None, None) => Cow::Borrowed("Model"),
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
    let width = settings::trigger_width(&label, ui::MODEL_TRIGGER_MAX, t);
    let open = settings::is_open(s, KEY);
    let current = picker.model_value().map(row_key);
    let trigger = ui::select_trigger(KEY, "Model", Some(&label), open, ui::MODEL_TRIGGER_MAX, t)
        .help(label)
        .on_press_tx(move |s: &mut Desktop, tx| {
            if settings::is_open(s, KEY) {
                close(s);
                return;
            }
            // The menu's own changes animate (see `menu`); its opening lands
            // at once, so the rows are opaque and pressable on the first frame.
            tx.instant();
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
    (trigger, width)
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
    label: &'a str,
    detail: Option<&'a str>,
}

/// The models matching the search, grouped by provider in the runner's
/// order.
fn groups<'a>(model: &'a ConfigOption, query: &str) -> Vec<(Option<&'a str>, Vec<Entry<'a>>)> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    // Disambiguate aliases even when the search hides their namesakes.
    let mut duplicates = HashMap::new();
    for choice in &model.choices {
        duplicates
            .entry((group(choice), model_name(choice)))
            .and_modify(|duplicate| *duplicate = true)
            .or_insert(false);
    }
    let mut groups: Vec<(Option<&str>, Vec<Entry<'a>>)> = Vec::new();
    for choice in &model.choices {
        let group = group(choice);
        let label = model_name(choice);
        if !words.is_empty() {
            let id = group.unwrap_or_default();
            let haystack =
                format!("{} {id} {label} {}", provider_name(id), choice.value).to_lowercase();
            if !words.iter().all(|word| haystack.contains(word.as_str())) {
                continue;
            }
        }
        let entry = Entry {
            value: &choice.value,
            label,
            detail: duplicates[&(group, label)].then(|| {
                choice
                    .value
                    .split_once('/')
                    .map_or(choice.value.as_str(), |(_, id)| id)
            }),
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
    let max_width = s.window_width - S16;
    let thinking = picker.levels.map(|levels| thinking(s, picker, levels, t));
    let width = thinking
        .as_ref()
        .map_or(MODEL_MENU_WIDTH, |(_, width)| *width);
    let mut body = col()
        .key("model-menu")
        .role("dialog")
        .label("Model")
        .w(width.max(MODEL_MENU_WIDTH).min(max_width))
        .min_h(0.0)
        .radius(Size::Default.radius())
        .fill(k.c.layer_01)
        .focus_return(format!("{KEY}-trigger"))
        .modal(true, close);
    if let Some(model) = picker.model {
        let (field, list) = models(s, picker, model, t);
        body = body
            .child(row().w_full().px(S4).pt(S4).instant_changes().child(field))
            .child(ui::rule(t))
            .child(list);
    }
    if let Some((section, _)) = thinking {
        if picker.model.is_some() {
            body = body.child(ui::rule(t));
        }
        body = body.child(section);
    }
    layer.push(scrim("model-scrim", close));
    // Mounted above the trigger as `rune_kit::popover::anchored` mounts a
    // surface, but without its `instant_changes`, which lands every change
    // inside it: a search edit's commit animates (`animate_edit_layout` on the
    // field), so the surface and the list's viewport spring to their new
    // height on one spring, bottom pinned to the trigger, while dropped rows
    // fade where they stood. Its opening lands at once instead (the trigger).
    let fade = Transition {
        insertion: Effect::IDENTITY,
        removal: Effect::opacity(0.0),
        springs: SpringPair::new(rune::tok::motion::INSTANT, Spring::new(0.25, 0.0)),
        ..Transition::identity()
    };
    let body = k
        .shadow_layers(&k.elevation.floating)
        .fold(body, El::box_shadow)
        .anchored_fit(format!("{KEY}-trigger"), Side::Top, Align::Start, S6)
        .transition(fade)
        .motion_intent(MotionIntent::CUT)
        .token_motion(k.motion.overlay);
    layer.push(body);
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
        .label("Models")
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
    let field = TextInput::new(&s.model_menu.query)
        .label("Search models")
        .placeholder("Search models")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Default.height())
        .active_descendant(active.as_deref().map(row_key))
        .on_edit(|s: &mut Desktop| &mut s.model_menu.query)
        // Typing commits instantly by default; the filtered list and the menu
        // around it should grow and shrink smoothly, dropped rows fading out.
        .animate_edit_layout()
        .on_change(|s: &mut Desktop, tx| {
            s.model_menu.active = None;
            tx.scroll_to_instant(LIST, 0.0, 0.0);
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
        .radius(Size::Default.radius())
        .hover(t.hover)
        .on_press_tx(move |s: &mut Desktop, tx| choose_model(s, tx, owner, &id, &value))
        .child(
            text(entry.label)
                .font(t.ui)
                .color(t.text)
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
                .min_w(0.0)
                .shrink()
                .truncate()
                .help_when_truncated(detail)
                .a11y_hidden()
        }))
        .child(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.muted)
                .opacity(if current { 1.0 } else { 0.0 })
                .a11y_hidden(),
        );
    if let Some(detail) = entry.detail {
        item = item.description(detail);
    }
    if active {
        item = item.fill(t.selected);
    }
    item
}

/// Keep all tiers on one line. The menu grows to fit their labels without
/// depending on the active tier, and the row scrolls if the window is narrower.
fn thinking(s: &Desktop, picker: &Picker<'_>, levels: Levels<'_>, t: &Theme) -> (El, f32) {
    let id = levels.id();
    let selected = s
        .model_menu
        .chosen
        .as_ref()
        .filter(|(option, value)| option == id && levels.has(value))
        .map(|(_, value)| value.as_str())
        .or_else(|| levels.value(picker.target));
    let owner = picker.target.owner();
    let name = ui::sentence_label(levels.name());
    let mut chips = row()
        .key("levels")
        .role("radiogroup")
        .label(name.as_ref())
        .w_full()
        .gap(S4)
        .focus_group()
        .scroll_x();
    let mut width = S16 - S4;
    for (value, choice) in levels.choices() {
        let on = selected == Some(value);
        let label = levels.choice_label(value, choice);
        let chip_width = rune::text::measure(&label, &[], t.ui_medium, None).0.ceil() + S16;
        width += chip_width + S4;
        let key = format!("level-{value}");
        let id = id.to_owned();
        let value = value.to_owned();
        chips = chips.child(
            row()
                .key(key)
                .role("radio")
                .label(label.as_ref())
                .checked(on)
                .w(chip_width)
                .h(Size::Small.height())
                .center()
                .radius(Size::Small.radius())
                .fill(if on {
                    t.selected
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(if on { t.selected } else { t.hover })
                .focusable(true)
                .focus_selected(on)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    if !on {
                        set_level(s, tx, owner, &id, &value);
                    }
                })
                .child(
                    text(label)
                        .font(t.ui_medium)
                        .color(if on { t.text } else { t.muted })
                        .a11y_hidden(),
                ),
        );
    }
    let section = col()
        .key("model-thinking")
        .w_full()
        .px(S8)
        .pt(S6)
        .pb(S8)
        .gap(S4)
        .child(ui::section(&name, t))
        .child(chips);
    (section, width)
}

/// Chooses thinking level `value` for option `id`, and shows it while the
/// menu stays open on this model (see [`ModelMenu::chosen`]).
fn set_level(s: &mut Desktop, tx: &mut Tx, owner: Owner, id: &str, value: &str) {
    s.model_menu.chosen = Some((id.to_owned(), value.to_owned()));
    settings::apply(
        s,
        tx,
        owner,
        id.to_owned(),
        OptionValue::Select(value.to_owned()),
    );
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

/// Selects model `value`, leaving the menu open on its thinking levels. A
/// session applies it and reports the new model's levels itself. A draft keeps
/// its thinking level only where the new model offers it, and asks the runner
/// for a model's levels it hasn't reported.
fn choose_model(s: &mut Desktop, tx: &mut Tx, owner: Owner, id: &str, value: &str) {
    s.model_menu.active = Some(value.to_owned());
    // The new model's levels are the runner's to report.
    s.model_menu.chosen = None;
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

/// The runner's provider group, or the provider prefix in a model ID.
fn group(choice: &OptionChoice) -> Option<&str> {
    choice
        .group
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| choice.value.split_once('/').map(|(provider, _)| provider))
}

fn model_name(choice: &OptionChoice) -> &str {
    let label = choice.name.trim();
    if label.is_empty() {
        &choice.value
    } else {
        label
    }
}

/// ACP runners can return either a human-readable group or a provider ID.
fn provider_name(group: &str) -> Cow<'_, str> {
    let known = match group {
        "openai" => "OpenAI",
        "openai-codex" => "OpenAI Codex",
        "openrouter" => "OpenRouter",
        "xai" => "xAI",
        "deepseek" => "DeepSeek",
        "github-copilot" => "GitHub Copilot",
        "google-vertex" => "Google Vertex",
        "google-gemini-cli" => "Gemini CLI",
        "amazon-bedrock" => "Amazon Bedrock",
        "azure-openai-responses" => "Azure OpenAI",
        "huggingface" => "Hugging Face",
        "zai" => "Z.ai",
        "opencode" => "OpenCode",
        "opencode-go" => "OpenCode Go",
        label if label.chars().any(char::is_uppercase) => label,
        id if id.contains(['-', '_']) => {
            let mut words = String::with_capacity(id.len());
            for word in id.split(['-', '_']).filter(|word| !word.is_empty()) {
                if !words.is_empty() {
                    words.push(' ');
                }
                words.push_str(&ui::sentence_label(word));
            }
            return Cow::Owned(words);
        }
        label => return ui::sentence_label(label),
    };
    Cow::Borrowed(known)
}
