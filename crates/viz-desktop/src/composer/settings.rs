use rune::{El, row};
use rune_kit::MenuItem;
use viz_client::protocol::{ConfigOption, ConversationId, OptionKind, OptionValue};

use crate::{
    app::{Desktop, NewChat},
    backend::Command,
    composer::workspace,
    theme::{S2, S32, Size, Theme},
    ui,
};

const CATEGORIES: [&str; 3] = ["model", "thought_level", "mode"];

fn category(option: &ConfigOption) -> Option<usize> {
    match option.category.as_deref() {
        Some("effort") => Some(1),
        Some(category) => CATEGORIES.iter().position(|name| *name == category),
        None => None,
    }
}

/// An inline select's chrome around its label: padding, gap and chevron.
const PICKER_CHROME: f32 = S32;
pub(super) const GAP: f32 = S2;

/// The width an inline select takes for `label`.
pub(super) fn picker_width(label: &str, t: &Theme) -> f32 {
    (rune::text::measure(label, &[], t.ui_medium, None).0 + PICKER_CHROME)
        .min(ui::PICKER_TRIGGER_MAX)
}

/// The draft or session's selects: model, effort and mode in that priority.
/// Options that don't fit fold into More with the uncategorized options.
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
        let (bar, width) = workspace::toolbar(s, chat, bar, t, layer);
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
    let mut primary: Vec<&ConfigOption> = options
        .iter()
        .filter(|option| category(option).is_some())
        .collect();
    primary.sort_by_key(|option| category(option));
    let extra = options.iter().filter(|option| category(option).is_none());
    let more = picker_width("More", t) + GAP;
    let mut used = 0.0;
    let mut shown = 0;
    for (index, option) in primary.iter().enumerate() {
        let width = picker_width(value_name(option, target), t) + GAP;
        // Room for More whenever something is left for it.
        let reserve = if index + 1 < primary.len() || extra.clone().next().is_some() {
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
    let (inline, folded) = primary.split_at(shown);
    for option in inline {
        let key = format!("option-{}", option.id);
        let menu = picker(s, &key, &option.name, Some(value_name(option, target)), t);
        let menu = if is_open(s, &key) {
            add_choices(menu, target, option)
        } else {
            menu
        };
        bar = bar.child(menu.build(&t.k, layer));
    }
    let mut overflow = folded.iter().copied().chain(extra).peekable();
    if overflow.peek().is_some() {
        let mut menu = picker(s, "more-options", "More", None, t);
        if is_open(s, "more-options") {
            for (index, option) in overflow.enumerate() {
                if index != 0 {
                    menu = menu.separator();
                }
                let label = if folded.iter().any(|folded| folded.id == option.id) {
                    format!("{} · {}", option.name, value_name(option, target))
                } else {
                    option.name.clone()
                };
                menu = add_choices(menu.group_label(&label), target, option);
            }
        }
        bar = bar.child(menu.build(&t.k, layer));
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
    let target = key.to_owned();
    ui::picker(
        key,
        name,
        value,
        is_open(s, key),
        move |s: &mut Desktop| {
            s.open_picker = if is_open(s, &target) {
                None
            } else {
                Some(target.clone())
            };
        },
        |s| s.open_picker = None,
        t,
    )
}

#[derive(Clone, Copy)]
enum Options<'a> {
    Draft(&'a NewChat),
    Conversation(ConversationId),
}

impl<'a> Options<'a> {
    fn value(self, option: &'a ConfigOption) -> &'a OptionValue {
        match self {
            Self::Draft(chat) => chat.options.get(&option.id).unwrap_or(&option.value),
            Self::Conversation(_) => &option.value,
        }
    }
}

fn value_name<'a>(option: &'a ConfigOption, target: Options<'a>) -> &'a str {
    match target.value(option) {
        OptionValue::Boolean(true) => "On",
        OptionValue::Boolean(false) => "Off",
        OptionValue::Select(value) => option
            .choices
            .iter()
            .find(|choice| choice.value == *value)
            .map_or(value, |choice| &choice.name),
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
                menu = menu.group_label(group);
            }
        }
        let mut item = choice(
            target,
            option,
            &entry.name,
            OptionValue::Select(entry.value.clone()),
        );
        if let Some(detail) = entry.description.as_deref() {
            item = item.description(detail);
        }
        menu = menu.item(item);
    }
    if option.choices.is_empty() {
        menu = menu.item(MenuItem::new("No choices").disabled(true));
    }
    menu
}

fn choice(target: Options<'_>, option: &ConfigOption, label: &str, value: OptionValue) -> MenuItem {
    let id = option.id.clone();
    let checked = *target.value(option) == value;
    let draft = match target {
        Options::Draft(chat) => Some(chat.id),
        Options::Conversation(_) => None,
    };
    let conversation = match target {
        Options::Draft(_) => None,
        Options::Conversation(id) => Some(id),
    };
    MenuItem::new(label)
        .checked(checked)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.open_picker = None;
            tx.focus("draft", false);
            if checked {
                return;
            }
            if let Some(conversation) = conversation {
                s.send(
                    Command::Configure {
                        conversation,
                        option: id.clone(),
                        value: value.clone(),
                    },
                    tx,
                );
            } else if let Some(chat) = s.new_chat.as_mut().filter(|chat| Some(chat.id) == draft) {
                chat.options.insert(id.clone(), value.clone());
            }
        })
}
