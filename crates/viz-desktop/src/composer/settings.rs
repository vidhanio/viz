use std::borrow::Cow;

use rune::{El, Tx, row};
use rune_kit::MenuItem;
use viz_client::protocol::{ConfigOption, ConversationId, OptionKind, OptionValue};

use super::{draft, model};
use crate::{
    app::{Desktop, NewChat},
    backend::Command,
    theme::{S2, S32, Size, Theme},
    ui,
};

/// What a runner's option is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Model,
    Thinking,
    Mode,
    Other,
}

pub(super) fn role(option: &ConfigOption) -> Role {
    match option.category.as_deref() {
        Some("model") => Role::Model,
        Some("thought_level" | "effort") => Role::Thinking,
        Some("mode") => Role::Mode,
        _ => Role::Other,
    }
}

/// An inline select's chrome around its label: padding, gap and chevron.
const PICKER_CHROME: f32 = S32;
pub(super) const GAP: f32 = S2;

/// The width an inline select takes for `label`, at most `max`.
pub(super) fn trigger_width(label: &str, max: f32, t: &Theme) -> f32 {
    (rune::text::measure(label, &[], t.ui_medium, None).0 + PICKER_CHROME).min(max)
}

/// The width an inline select takes for `label`.
pub(super) fn picker_width(label: &str, t: &Theme) -> f32 {
    trigger_width(label, ui::PICKER_TRIGGER_MAX, t)
}

/// The draft or session's selects: the model picker (model and thinking),
/// then the mode. Options that don't fit fold into More with the
/// uncategorized options.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>, budget: f32) -> El {
    let bar = row()
        .key("settings")
        .h(Size::Default.height())
        .gap(GAP)
        .center_y();
    if !s.connected() {
        return bar;
    }
    let (bar, budget, target, options) = if let Some(chat) = &s.new_chat {
        let (bar, width) = draft::toolbar(s, chat, bar, t, layer);
        let Some(harness) = chat.active_harness(&s.harnesses).filter(|_| !chat.sending) else {
            return bar;
        };
        (
            bar,
            budget - width,
            Options::Draft(chat),
            harness.options.as_slice(),
        )
    } else if let Some((conversation, state)) = s.selected.zip(s.conversation.as_ref()) {
        (
            bar,
            budget,
            Options::Conversation(conversation),
            state.options.as_slice(),
        )
    } else {
        return bar;
    };
    session_options(s, t, layer, budget, bar, target, options)
}

fn session_options(
    s: &Desktop,
    t: &Theme,
    layer: &mut Vec<El>,
    budget: f32,
    mut bar: El,
    target: Options<'_>,
    options: &[ConfigOption],
) -> El {
    let mut used = 0.0;
    if let Some(picker) = model::Picker::new(target, options) {
        let (picker, width) = model::picker(s, &picker, t, layer);
        bar = bar.child(picker);
        used += width + GAP;
    }
    let modes: Vec<_> = options
        .iter()
        .filter(|option| role(option) == Role::Mode)
        .collect();
    let extra = options.iter().filter(|option| role(option) == Role::Other);
    let more = picker_width("More", t) + GAP;
    let mut shown = 0;
    for (index, option) in modes.iter().enumerate() {
        let width = picker_width(&value_name(option, target), t) + GAP;
        // Room for More whenever something is left for it.
        let reserve = if index + 1 < modes.len() || extra.clone().next().is_some() {
            more
        } else {
            0.0
        };
        if used + width + reserve > budget {
            break;
        }
        used += width;
        shown += 1;
    }
    let (inline, folded) = modes.split_at(shown);
    for option in inline {
        let key = format!("option-{}", option.id);
        let menu = picker(
            s,
            &key,
            &ui::sentence_label(&option.name),
            Some(&value_name(option, target)),
            t,
        );
        let menu = if is_open(s, &key) {
            add_choices(menu, target, option)
        } else {
            menu
        };
        bar = bar.child(menu.build(&t.overlay, layer));
    }
    let mut overflow = folded.iter().copied().chain(extra).peekable();
    if overflow.peek().is_some() {
        let mut menu = picker(s, "more-options", "More", None, t);
        if is_open(s, "more-options") {
            for (index, option) in overflow.enumerate() {
                if index != 0 {
                    menu = menu.separator();
                }
                let name = ui::sentence_label(&option.name);
                let label = if folded.iter().any(|folded| folded.id == option.id) {
                    Cow::Owned(format!("{name} · {}", value_name(option, target)))
                } else {
                    name
                };
                menu = add_choices(menu.group_label(label), target, option);
            }
        }
        bar = bar.child(menu.build(&t.overlay, layer));
    }
    bar
}

pub(super) fn is_open(s: &Desktop, key: &str) -> bool {
    s.open_picker.as_deref() == Some(key)
}

pub(super) fn picker(
    s: &Desktop,
    key: &str,
    name: &str,
    value: Option<&str>,
    t: &Theme,
) -> rune_kit::Menu {
    ui::picker(key, name, value, is_open(s, key), toggle(key), close, t)
}

/// A picker whose trigger shows `mark` before `value` (a harness's icon).
pub(super) fn marked_picker(
    s: &Desktop,
    key: &str,
    name: &str,
    value: &str,
    mark: &'static rune::Icon,
    t: &Theme,
) -> rune_kit::Menu {
    let open = is_open(s, key);
    let trigger = ui::marked_trigger(key, name, value, mark, open, t).on_press(toggle(key));
    ui::picker_menu(key, trigger, open, close)
}

/// Opens picker `key`, or closes it when open.
fn toggle(key: &str) -> impl Fn(&mut Desktop) + 'static {
    let target = key.to_owned();
    move |s: &mut Desktop| {
        s.open_picker = if is_open(s, &target) {
            None
        } else {
            Some(target.clone())
        };
    }
}

fn close(s: &mut Desktop) {
    s.open_picker = None;
}

/// Whose options the composer shows: the fresh composer's draft (sent with
/// its first message) or a session's (applied at once).
#[derive(Clone, Copy)]
pub(super) enum Options<'a> {
    Draft(&'a NewChat),
    Conversation(ConversationId),
}

/// [`Options`] without the borrow, for a choice's handler.
#[derive(Clone, Copy)]
pub(super) enum Owner {
    Draft(u64),
    Conversation(ConversationId),
}

impl<'a> Options<'a> {
    /// The option's value: the draft's choice, else the runner's.
    pub(super) fn value(self, option: &'a ConfigOption) -> &'a OptionValue {
        match self {
            Self::Draft(chat) => chat.options.get(&option.id).unwrap_or(&option.value),
            Self::Conversation(_) => &option.value,
        }
    }

    pub(super) const fn owner(self) -> Owner {
        match self {
            Self::Draft(chat) => Owner::Draft(chat.id),
            Self::Conversation(id) => Owner::Conversation(id),
        }
    }
}

/// Sets option `id` to `value`: on the session at once, or in the draft.
pub(super) fn apply(s: &mut Desktop, tx: &mut Tx, owner: Owner, id: String, value: OptionValue) {
    match owner {
        Owner::Conversation(conversation) => {
            s.send(
                Command::Configure {
                    conversation,
                    option: id,
                    value,
                },
                tx,
            );
        }
        Owner::Draft(draft) => {
            if let Some(chat) = s.new_chat.as_mut().filter(|chat| chat.id == draft) {
                chat.options.insert(id, value);
            }
        }
    }
}

/// The current choice's display name, not its wire value.
fn value_name<'a>(option: &'a ConfigOption, target: Options<'a>) -> Cow<'a, str> {
    match target.value(option) {
        OptionValue::Boolean(true) => Cow::Borrowed("On"),
        OptionValue::Boolean(false) => Cow::Borrowed("Off"),
        OptionValue::Select(value) => ui::sentence_label(
            option
                .choices
                .iter()
                .find(|choice| choice.value == *value)
                .map_or(value, |choice| &choice.name),
        ),
    }
}

fn add_choices(
    mut menu: rune_kit::Menu,
    target: Options<'_>,
    option: &ConfigOption,
) -> rune_kit::Menu {
    if option.kind == OptionKind::Boolean {
        return [false, true].into_iter().fold(menu, |menu, value| {
            menu.item(choice(
                target,
                option,
                if value { "On" } else { "Off" },
                OptionValue::Boolean(value),
            ))
        });
    }
    let mut group = None;
    for entry in &option.choices {
        if group != entry.group.as_deref() {
            group = entry.group.as_deref();
            if let Some(group) = group {
                menu = menu.group_label(ui::sentence_label(group));
            }
        }
        // Names only: runners' descriptions (Codex's modes) are long prose that
        // a compact menu would only truncate.
        menu = menu.item(choice(
            target,
            option,
            &ui::sentence_label(&entry.name),
            OptionValue::Select(entry.value.clone()),
        ));
    }
    if option.choices.is_empty() {
        menu = menu.item(MenuItem::new("No choices available").disabled(true));
    }
    menu
}

fn choice(target: Options<'_>, option: &ConfigOption, label: &str, value: OptionValue) -> MenuItem {
    let id = option.id.clone();
    let checked = *target.value(option) == value;
    let owner = target.owner();
    MenuItem::new(label)
        .checked(checked)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.open_picker = None;
            tx.focus("draft", false);
            if !checked {
                apply(s, tx, owner, id.clone(), value.clone());
            }
        })
}
