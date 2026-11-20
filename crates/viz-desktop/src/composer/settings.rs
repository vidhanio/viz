use rune::{El, Tx, row};
use rune_kit::MenuItem;
use viz_client::protocol::{ConfigOption, OptionKind, OptionValue, ThreadId};

use super::{draft, model};
use crate::{
    app::{Desktop, NewThread},
    backend::Command,
    theme::{S2, Size, Theme},
    ui,
};

/// What a runner's option is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Model,
    Thinking,
    Hidden,
    Other,
}

pub(super) fn role(option: &ConfigOption) -> Role {
    match option.category.as_deref() {
        Some("model") => Role::Model,
        Some("thought_level" | "effort") => Role::Thinking,
        Some("mode" | "permission" | "permissions") => Role::Hidden,
        _ => Role::Other,
    }
}

const GAP: f32 = S2;

/// The model picker and other runner settings, excluding permission modes.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let bar = row()
        .key("settings")
        .h(Size::Default.height())
        .gap(GAP)
        .center_y();
    if !s.connected() {
        return bar;
    }
    let (bar, target, options) = if let Some(thread) = &s.new_thread {
        let bar = draft::toolbar(s, thread, bar, t, layer);
        let Some(harness) = thread
            .active_harness(&s.harnesses)
            .filter(|_| !thread.sending)
        else {
            return bar;
        };
        (bar, Options::Draft(thread), harness.options.as_slice())
    } else if let Some((thread, state)) = s.selected.zip(s.state.as_ref()) {
        (bar, Options::Thread(thread), state.options.as_slice())
    } else {
        return bar;
    };
    session_options(s, t, layer, bar, target, options)
}

fn session_options(
    s: &Desktop,
    t: &Theme,
    layer: &mut Vec<El>,
    mut bar: El,
    target: Options<'_>,
    options: &[ConfigOption],
) -> El {
    if let Some(picker) = model::Picker::new(target, options) {
        bar = bar.child(model::picker(s, &picker, t, layer));
    }
    let mut overflow = options
        .iter()
        .filter(|option| role(option) == Role::Other)
        .peekable();
    if overflow.peek().is_some() {
        let mut menu = picker(s, "more-options", "More", None, t);
        if is_open(s, "more-options") {
            for (index, option) in overflow.enumerate() {
                if index != 0 {
                    menu = menu.separator();
                }
                menu = add_choices(
                    menu.group_label(ui::sentence_label(&option.name)),
                    target,
                    option,
                );
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
    Draft(&'a NewThread),
    Thread(ThreadId),
}

/// [`Options`] without the borrow, for a choice's handler.
#[derive(Clone, Copy)]
pub(super) enum Owner {
    Draft(u64),
    Thread(ThreadId),
}

impl<'a> Options<'a> {
    /// The option's value: the draft's choice, else the runner's.
    pub(super) fn value(self, option: &'a ConfigOption) -> &'a OptionValue {
        match self {
            Self::Draft(thread) => thread.options.get(&option.id).unwrap_or(&option.value),
            Self::Thread(_) => &option.value,
        }
    }

    pub(super) const fn owner(self) -> Owner {
        match self {
            Self::Draft(thread) => Owner::Draft(thread.id),
            Self::Thread(id) => Owner::Thread(id),
        }
    }
}

/// Sets option `id` to `value`: on the session at once, or in the draft.
pub(super) fn apply(s: &mut Desktop, tx: &mut Tx, owner: Owner, id: String, value: OptionValue) {
    match owner {
        Owner::Thread(thread) => {
            s.send(
                Command::Configure {
                    thread,
                    option: id,
                    value,
                },
                tx,
            );
        }
        Owner::Draft(draft) => {
            if let Some(thread) = s.new_thread.as_mut().filter(|thread| thread.id == draft) {
                thread.options.insert(id, value);
            }
        }
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
