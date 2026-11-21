//! The model picker: one menu for the runner's model and its thinking level.
//! The trigger names both. The menu groups models by provider and ends with
//! the selected model's thinking levels in a fixed-height footer so loading or
//! switching models never shifts the layout. Search matches both display names
//! and IDs; arrows move the highlight, and Enter chooses the model.

use std::{borrow::Cow, collections::HashMap};

use rune::{
    Align, Color, Effect, El, Key, KeyEvent, MotionIntent, ScrollSpec, Side, Spring, SpringPair,
    TextEdit, Transition, Tx, col, el, icon, row, text,
};
use rune_kit::{TextInput, scrim};
use viz_client::protocol::{
    ConfigOption, DependentOption, Harness, OptionChoice, OptionKind, OptionValue,
};

use super::settings::{self, Options, Owner, Role, role};
use crate::{
    app::Desktop,
    server::Command,
    theme::{
        HAIRLINE, ICON_SMALL, MODEL_LIST_HEIGHT, MODEL_MENU_WIDTH, RADIUS_CARD, RADIUS_CONTROL, S2,
        S4, S6, S8, S16, S24, Size, Theme,
    },
    ui,
};

/// The picker's `open_picker` value and key.
const KEY: &str = "model";
const FIELD: &str = "model-query";
const LIST: &str = "model-list";

/// Visual tier for a thinking/effort choice, distinguishing `Xhigh` and `Max`
/// with their own icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effort {
    Off,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    fn classify(value: &str, name: &str, index: usize, total: usize) -> Self {
        let lower_val = value.to_ascii_lowercase();
        let lower_name = name.to_ascii_lowercase();
        let has = |needle: &str| lower_val.contains(needle) || lower_name.contains(needle);
        let exact = |needle: &str| lower_val == needle || lower_name == needle;
        if exact("none") || exact("off") || exact("disabled") {
            Self::Off
        } else if has("xhigh") || has("x-high") || has("extra-high") || has("extra high") {
            Self::Xhigh
        } else if exact("max") || has("maximum") || has("ultra") || lower_val.ends_with("max") {
            Self::Max
        } else if has("min") || has("low") {
            Self::Low
        } else if has("high") {
            Self::High
        } else if has("med") || has("default") || has("auto") || total <= 1 {
            Self::Medium
        } else {
            match ((index * 3) / total.max(1) + 1).min(3) {
                0 => Self::Off,
                1 => Self::Low,
                2 => Self::Medium,
                _ => Self::High,
            }
        }
    }
}

/// The open menu's search, keyboard highlight, pending thinking choice, and
/// loading state while a newly selected model's thinking levels arrive.
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
    /// Model value whose thinking levels are currently being fetched from the
    /// server.
    loading_levels: Option<String>,
}

impl ModelMenu {
    /// Clears any pending thinking-levels load once fresh options arrive.
    pub fn clear_loading(&mut self) {
        self.loading_levels = None;
    }
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
            Options::Draft(thread) => thread.options.get(self.id()),
            Options::Thread(_) => None,
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

    fn model_label(&self) -> Option<&'a str> {
        self.model.zip(self.model_value()).map(|(model, value)| {
            model
                .choices
                .iter()
                .find(|choice| choice.value == value)
                .map_or(value, model_name)
        })
    }

    /// The trigger's accessible label.
    fn label(&self) -> Cow<'a, str> {
        let model = self.model_label();
        let level = self.effort();
        match (model, level) {
            (Some(model), Some((_, level, _))) => Cow::Owned(format!("{model} · {level}")),
            (Some(model), None) => Cow::Borrowed(model),
            (None, Some((levels, level, _))) => {
                Cow::Owned(format!("{} · {level}", ui::sentence_label(levels.name())))
            }
            (None, None) => Cow::Borrowed("Model"),
        }
    }

    /// Selected thinking level plus its visual [`Effort`] tier.
    fn effort(&self) -> Option<(Levels<'a>, Cow<'a, str>, Effort)> {
        let levels = self.levels?;
        let value = levels.value(self.target)?;
        let label = levels.label(value);
        let choices: Vec<_> = levels.choices().collect();
        let total = choices.len().max(1);
        let index = choices
            .iter()
            .position(|(choice, _)| *choice == value)
            .unwrap_or(0);
        let effort = Effort::classify(value, label.as_ref(), index, total);
        Some((levels, label, effort))
    }
}

/// Signal bars for `Off`..`High`, lightning bolt for `Xhigh`, and flame for
/// `Max`.
fn effort_glyph(effort: Effort, on_accent: bool, t: &Theme) -> El {
    match effort {
        Effort::Xhigh => icon(&ui::ZAP, ICON_SMALL)
            .color(if on_accent { t.on_accent } else { t.steer })
            .a11y_hidden(),
        Effort::Max => icon(&ui::FLAME, ICON_SMALL)
            .color(if on_accent { t.on_accent } else { t.danger })
            .a11y_hidden(),
        Effort::Off | Effort::Low | Effort::Medium | Effort::High => {
            let bars = match effort {
                Effort::Off => 0,
                Effort::Low => 1,
                Effort::Medium => 2,
                Effort::High => 3,
                Effort::Xhigh | Effort::Max => unreachable!(),
            };
            let lit = if on_accent { t.on_accent } else { t.text };
            let dim = if on_accent {
                t.on_accent.alpha(0.35)
            } else {
                t.hairline
            };
            row().gap(1.5).items_end().h(10.0).a11y_hidden().children(
                [4.0, 7.0, 10.0].into_iter().enumerate().map(|(i, height)| {
                    el().w(S2)
                        .h(height)
                        .radius(HAIRLINE)
                        .fill(if i < bars { lit } else { dim })
                }),
            )
        }
    }
}

/// The trigger and the menu while it is open.
pub(super) fn picker(s: &Desktop, picker: &Picker<'_>, t: &Theme, layer: &mut Vec<El>) -> El {
    let full_label = picker.label();
    let open = settings::is_open(s, KEY);
    let current = picker.model_value().map(row_key);
    let model_text = picker.model_label();
    let effort = picker.effort();
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
        trigger = trigger.child(effort_glyph(tier, false, t)).child(
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
    let model_parts = picker.model.map(|model| models(s, picker, model, t));
    let loading_levels = s.model_menu.loading_levels.is_some();
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
        body = body.child(thinking_section(s, picker, loading_levels, t));
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
fn models(s: &Desktop, picker: &Picker<'_>, model: &ConfigOption, t: &Theme) -> (El, El) {
    let current = picker.model_value();
    let groups = groups(model, &s.model_menu.query.text);
    let values: Vec<String> = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter().map(|entry| entry.value.to_owned()))
        .collect();
    let current_visible =
        current.is_some_and(|current| values.iter().any(|value| value == current));
    let active = s
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
    let field = TextInput::new(&s.model_menu.query)
        .label("Search models")
        .placeholder("Search models")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Default.height())
        .active_descendant(active.as_deref().map(row_key))
        .on_edit(|s: &mut Desktop| &mut s.model_menu.query)
        .animate_edit_layout()
        .on_change(|s: &mut Desktop, tx| {
            s.model_menu.active = None;
            tx.scroll_to_instant(LIST, 0.0, 0.0);
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
        .radius(RADIUS_CONTROL)
        .fill(if active {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover_group()
        .on_press_tx(move |s: &mut Desktop, tx| choose_model(s, tx, owner, &id, &value))
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

/// Fixed-height thinking footer: shows a loading row while fetching a model's
/// thinking levels, a quiet placeholder when a model has none, or filled tier
/// chips on one line. Keeping its height constant prevents layout jumps.
fn thinking_section(s: &Desktop, picker: &Picker<'_>, loading: bool, t: &Theme) -> El {
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
        thinking_chips(s, picker, levels, &heading, t)
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

fn thinking_chips(
    s: &Desktop,
    picker: &Picker<'_>,
    levels: Levels<'_>,
    name: &str,
    t: &Theme,
) -> El {
    let id = levels.id();
    let selected = s
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
        let tier = Effort::classify(value, label.as_ref(), index, total);
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
                        set_level(s, tx, owner, &id, &value);
                    }
                })
                .child(effort_glyph(tier, on, t))
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
            let chosen = &values[index];
            if committed == Some(chosen.as_str()) {
                close(s);
                tx.focus("draft", false);
            } else {
                choose_model(s, tx, owner, id, chosen);
            }
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
    s.model_menu.chosen = None;
    let chosen = OptionValue::Select(value.to_owned());
    let draft = match owner {
        Owner::Thread(_) => {
            let changed = s.state.as_ref().is_some_and(|state| {
                state
                    .options
                    .iter()
                    .any(|option| option.id == id && option.value != chosen)
            });
            if changed {
                s.model_menu.loading_levels = Some(value.to_owned());
                settings::apply(s, tx, owner, id.to_owned(), chosen);
            }
            return;
        }
        Owner::Draft(draft) => draft,
    };
    let Some(thread) = s.new_thread.as_mut().filter(|thread| thread.id == draft) else {
        return;
    };
    thread.options.insert(id.to_owned(), chosen);
    let Some(harness) = thread.active_harness(&s.harnesses) else {
        return;
    };
    let (stale, fetch) = match Picker::new(Options::Draft(thread), &harness.options) {
        Some(Picker {
            levels: Some(levels),
            ..
        }) => {
            let kept = matches!(
                thread.options.get(levels.id()),
                Some(OptionValue::Select(level)) if levels.has(level)
            );
            ((!kept).then(|| vec![levels.id().to_owned()]), None)
        }
        _ if unseen(harness, id, value) => (Some(thinking_ids(harness)), Some(harness.id.clone())),
        _ => (None, None),
    };
    for level in stale.into_iter().flatten() {
        thread.options.remove(&level);
    }
    if let Some(harness) = fetch {
        s.model_menu.loading_levels = Some(value.to_owned());
        s.send(
            Command::HarnessOptions {
                harness,
                model: value.to_owned(),
            },
            tx,
        );
    } else {
        s.model_menu.loading_levels = None;
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
            Role::Hidden | Role::Other => Vec::new(),
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
