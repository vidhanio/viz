use rune::{El, TextStyle, row};
use rune_kit::MenuItem;
use viz_client::protocol::{ConfigOption, ConversationId, OptionKind, OptionValue};

use crate::{app::Desktop, backend::Command, theme::Theme, ui};

const CATEGORIES: [&str; 4] = ["model", "thought_level", "mode", "access"];
const HARNESS_PICKER: &str = "harness";

fn category(option: &ConfigOption) -> Option<usize> {
    match option.category.as_deref() {
        Some("effort") => Some(1),
        Some(category) => CATEGORIES.iter().position(|name| *name == category),
        None => None,
    }
}

/// An inline select's chrome around its label: padding, gap and chevron.
const PICKER_CHROME: f32 = 32.0;
/// The kit caps an inline select's width here and truncates the label.
const PICKER_MAX: f32 = 154.0;
const GAP: f32 = 2.0;

/// The width an inline select takes for `label`.
fn picker_width(label: &str, t: &Theme) -> f32 {
    let font = TextStyle {
        line_height: 16.0,
        ..t.k.ty.ui_medium
    };
    (rune::text::measure(label, &[], font, None).0 + PICKER_CHROME).min(PICKER_MAX)
}

/// The session's selects in one row no wider than `budget`: model, effort, mode
/// and access in that priority, the ones that don't fit folded into More with
/// the uncategorized options.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>, budget: f32) -> El {
    let bar = row().key("settings").min_h(26.0).gap(2.0).center_y();
    if let Some(new) = &s.new_chat {
        return harness_bar(s, new.request.is_some(), bar, t, layer);
    }
    let mut bar = bar;
    let Some((conversation, state)) = s.selected.zip(s.conversation.as_ref()) else {
        return bar;
    };
    if !s.connected() {
        return bar;
    }
    let mut primary: Vec<&ConfigOption> = state
        .options
        .iter()
        .filter(|option| category(option).is_some())
        .collect();
    primary.sort_by_key(|option| category(option));
    let extra = state
        .options
        .iter()
        .filter(|option| category(option).is_none());
    let more = picker_width("More", t) + GAP;
    let mut used = 0.0;
    let mut shown = 0;
    for (index, option) in primary.iter().enumerate() {
        let width = picker_width(value_name(option), t) + GAP;
        // Room for More whenever something is left for it.
        let reserve = if index + 1 < primary.len() || extra.clone().next().is_some() {
            more
        } else {
            0.0
        };
        // The model always shows; it's what a chat is mostly about.
        if index > 0 && used + width + reserve > budget {
            break;
        }
        used += width;
        shown += 1;
    }
    let (inline, folded) = primary.split_at(shown);
    for option in inline {
        let key = format!("option-{}", option.id);
        let menu = picker(s, &key, &option.name, Some(value_name(option)), t);
        let menu = if is_open(s, &key) {
            add_choices(menu, conversation, option)
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
                    format!("{} · {}", option.name, value_name(option))
                } else {
                    option.name.clone()
                };
                menu = add_choices(menu.group_label(&label), conversation, option);
            }
        }
        bar = bar.child(menu.build(&t.k, layer));
    }
    bar
}

/// A fresh composer's row: the runner to bind, or the binding in flight.
fn harness_bar(s: &Desktop, opening: bool, bar: El, t: &Theme, layer: &mut Vec<El>) -> El {
    use rune_kit::ButtonVariant;
    if opening {
        return bar
            .child(ui::spinner(13.0, t).ml(5.0))
            .child(ui::section("Opening session", t));
    }
    if s.harnesses.is_empty() {
        return bar.child(ui::section("No harness configured", t).ml(6.0));
    }
    if s.harnesses.len() == 1 {
        return bar.child(
            ui::button("Open session", ButtonVariant::Ghost)
                .disabled(!s.connected())
                .on_press_tx(|s: &mut Desktop, tx| s.bind_harness(None, tx))
                .build(&t.k),
        );
    }
    if !s.connected() {
        return bar;
    }
    let menu = picker(s, HARNESS_PICKER, "Choose harness", None, t);
    let menu = if is_open(s, HARNESS_PICKER) {
        ui::menu_items(
            menu,
            s.harnesses.iter().map(|harness| {
                let name = harness.name.clone();
                let mut item = MenuItem::new(&name);
                if harness.default {
                    item = item.badge("Default");
                }
                item.on_select_tx(move |s: &mut Desktop, tx| {
                    s.bind_harness(Some(name.clone()), tx);
                })
            }),
        )
    } else {
        menu
    };
    bar.child(menu.build(&t.k, layer))
}

fn is_open(s: &Desktop, key: &str) -> bool {
    s.open_picker.as_deref() == Some(key)
}

fn picker(s: &Desktop, key: &str, name: &str, value: Option<&str>, t: &Theme) -> rune_kit::Menu {
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

fn value_name(option: &ConfigOption) -> &str {
    match &option.value {
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
    conversation: ConversationId,
    option: &ConfigOption,
) -> rune_kit::Menu {
    if option.kind == OptionKind::Boolean {
        return [false, true].into_iter().fold(menu, |menu, value| {
            menu.item(choice(
                conversation,
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
            conversation,
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

fn choice(
    conversation: ConversationId,
    option: &ConfigOption,
    label: &str,
    value: OptionValue,
) -> MenuItem {
    let id = option.id.clone();
    let checked = option.value == value;
    MenuItem::new(label)
        .checked(checked)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.open_picker = None;
            tx.focus("draft", false);
            if !checked {
                s.send(
                    Command::Configure {
                        conversation,
                        option: id.clone(),
                        value: value.clone(),
                    },
                    tx,
                );
            }
        })
}
