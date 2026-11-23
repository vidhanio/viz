//! Composer settings bar: model picker, overflow runner options, and context
//! window usage meter.

use rune::{El, row, text};
use rune_kit::{MenuItem, ProgressCircle};
use viz_client::protocol::{ConfigOption, ContextUsage, OptionKind, OptionValue};

use super::{draft, model};
use crate::{
    app::Desktop,
    model::{ModelPicker, OptionRole, OptionTarget, option_role},
    theme::{self, ICON_SMALL, READING_INSET, READING_WIDTH, S2, S4, S8, Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Drop the numeric context label below this composer width, keeping the ring
/// and tooltip.
const CONTEXT_TEXT_MIN_WIDTH: f32 = 520.0;
const GAP: f32 = S2;

/// The model picker and other runner settings, excluding permission modes.
pub(super) fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let bar = row()
        .key("settings")
        .h(Size::Default.height())
        .gap(GAP)
        .center_y();
    if !m.connected() {
        return bar;
    }
    let (bar, target, options, usage) = if let Some(thread) = &m.selection.new_thread {
        let bar = draft::toolbar(s, thread, bar, t, layer);
        let Some(harness) = thread
            .active_harness(m.harnesses())
            .filter(|_| !thread.sending)
        else {
            return bar;
        };
        (
            bar,
            OptionTarget::Draft(thread),
            harness.options.as_slice(),
            None,
        )
    } else if let Some((thread, state)) = m.selection.selected.zip(m.thread_state()) {
        (
            bar,
            OptionTarget::Thread(thread),
            state.options.as_slice(),
            state.usage.as_ref(),
        )
    } else {
        return bar;
    };
    let bar = session_options(s, t, layer, bar, target, options);
    let Some(usage) = usage.and_then(|usage| context_usage(s, usage, t)) else {
        return bar;
    };
    bar.child(rune::spacer()).child(usage)
}

fn context_usage(s: &Desktop, usage: &ContextUsage, t: &Theme) -> Option<El> {
    if usage.size == 0 {
        return None;
    }
    let percent = used_percent(usage.used, usage.size);
    let label = format!(
        "Context: {percent} used, {} of {} tokens",
        usage.used, usage.size
    );
    let used = u128::from(usage.used.min(usage.size));
    let size = u128::from(usage.size.max(1));
    let ink = if used * 100 >= size * 85 {
        t.danger
    } else if used * 100 >= size * 65 {
        t.warning
    } else {
        t.muted
    };
    let mut ring_kit = t.k.clone();
    ring_kit.c.icon_muted = ink;
    let reading_width = s.model.panels.window_width - s.widgets.sidebar_pane.shown_width();
    let composer_width = READING_INSET
        .mul_add(-2.0, reading_width)
        .min(READING_WIDTH);
    let show_text = composer_width >= CONTEXT_TEXT_MIN_WIDTH;
    let total = compact_tokens(usage.size);
    Some(
        row()
            .key("context-usage")
            .center_y()
            .gap(S4)
            .ml(S8)
            .mr(S4)
            .role("group")
            .label(&label)
            .help(label)
            .child(
                ProgressCircle::new(theme::fraction(usage.used, usage.size))
                    .size(ICON_SMALL + S2)
                    .build(&ring_kit)
                    .a11y_hidden(),
            )
            .children(show_text.then(|| {
                row()
                    .center_y()
                    .a11y_hidden()
                    .child(text(percent).font(t.mono_small).color(ink))
                    .child(text(format!("/{total}")).font(t.mono_small).color(t.faint))
            })),
    )
}

/// Whole percent of the window used, rounded half up; any use below half a
/// percent reads `<1%` rather than an empty-looking `0%`.
fn used_percent(used: u64, size: u64) -> String {
    let used = u128::from(used.min(size));
    let size = u128::from(size.max(1));
    match (used * 200 + size) / (size * 2) {
        0 if used > 0 => "<1%".to_owned(),
        whole => format!("{whole}%"),
    }
}

/// Round to one decimal, promoting a rounded 1000k to 1m.
fn compact_tokens(count: u64) -> String {
    let (unit, suffix) = match count {
        0..1_000 => return count.to_string(),
        1_000..999_950 => (1_000, 'k'),
        _ => (1_000_000, 'm'),
    };
    let step = unit / 10;
    let tenths = count / step + u64::from(count % step >= step / 2);
    let whole = tenths / 10;
    match tenths % 10 {
        0 => format!("{whole}{suffix}"),
        digit => format!("{whole}.{digit}{suffix}"),
    }
}

fn session_options(
    s: &Desktop,
    t: &Theme,
    layer: &mut Vec<El>,
    mut bar: El,
    target: OptionTarget<'_>,
    options: &[ConfigOption],
) -> El {
    if let Some(picker) = ModelPicker::new(target, options) {
        bar = bar.child(model::picker(s, &picker, t, layer));
    }
    let mut overflow = options
        .iter()
        .filter(|option| option_role(option) == OptionRole::Other)
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
    s.model.composer.open_picker.as_deref() == Some(key)
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

fn toggle(key: &str) -> impl Fn(&mut Desktop) + 'static {
    let target = key.to_owned();
    move |s: &mut Desktop| {
        s.dispatch_local(Msg::Composer(ComposerMsg::TogglePicker(target.clone())));
    }
}

fn close(s: &mut Desktop) {
    s.dispatch_local(Msg::Composer(ComposerMsg::ClosePicker));
}

fn add_choices(
    mut menu: rune_kit::Menu,
    target: OptionTarget<'_>,
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

fn choice(
    target: OptionTarget<'_>,
    option: &ConfigOption,
    label: &str,
    value: OptionValue,
) -> MenuItem {
    let id = option.id.clone();
    let checked = *target.value(option) == value;
    let owner = target.owner();
    MenuItem::new(label)
        .checked(checked)
        .on_select_tx(move |s: &mut Desktop, tx| {
            if checked {
                s.dispatch(Msg::Composer(ComposerMsg::ClosePicker), tx);
            } else {
                s.dispatch(
                    Msg::Composer(ComposerMsg::ApplyOption {
                        owner,
                        option_id: id.clone(),
                        value: value.clone(),
                    }),
                    tx,
                );
            }
        })
}
