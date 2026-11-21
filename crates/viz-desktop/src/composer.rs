//! Per-thread composer, settings, completions, and queued input.

mod completion;
mod draft;
mod model;
mod settings;
mod tasks;

pub use completion::{Completion, poll};
pub use model::{ModelMenu, preview as preview_model_menu};
use rune::{
    Color, Cursor, El, GrowthSpec, Key, KeyEvent, Phase, ScrollSpec, Spring, SpringPair,
    Transition, Tx, col, el, icon, row, text, text_input,
};
pub use tasks::DOCK_KEY as TASKS_DOCK_KEY;
use viz_client::protocol::{QueuedInput, RunId, SendMode, SteerQueuedInput, ThreadId};

use crate::{
    app::Desktop,
    attachments,
    server::Command,
    theme::{
        CARD_INSET, GLASS_BLUR, HAIRLINE, ICON_SMALL, RADIUS_COMPOSER, RADIUS_CONTROL,
        RADIUS_INNER, S2, S4, S6, S8, S12, S16, S24, S32, S48, Size, Theme,
    },
    ui,
};

const MAX_LINES: usize = 8;
/// Horizontal padding inside the composer's bottom controls bar.
const TOOLBAR_PAD_X: f32 = 10.0;
/// Minimum height of the composer's bottom controls bar (`30pt` circle + `8pt`
/// bottom padding).
const TOOLBAR_HEIGHT: f32 = 38.0;
/// Diameter of the primary circular action button.
const PRIMARY_CIRCLE: f32 = 30.0;
/// Diameter of the secondary circular Stop ring beside Queue/Steer.
const SECONDARY_CIRCLE: f32 = 28.0;
/// Side length of the rounded square stop glyph inside the circular button.
const STOP_GLYPH: f32 = 9.0;
/// The gap between the composer toolbar's items.
const TOOLBAR_GAP: f32 = S4;
/// Maximum queued items shown before offering `"Show all N"`.
const QUEUE_PREVIEW_LIMIT: usize = 3;
/// The tallest the queue list gets before scrolling when expanded.
const QUEUE_HEIGHT: f32 = 152.0;
/// Bottom clearance reserved in the scrolling transcript when the composer is
/// mounted.
const BASE_CLEARANCE: f32 = 148.0;
/// Extra clearance per queued row above the composer.
const QUEUE_ROW_HEIGHT: f32 = S32;

/// Bottom padding the transcript needs so its latest message scrolls completely
/// clear of the floating composer stack.
pub fn clearance(s: &Desktop) -> f32 {
    if s.selected.is_none() && s.new_thread.is_none() {
        return S24;
    }
    let queue_len = s.state.as_ref().map_or(0, |state| state.queue.len());
    let queue_extra = if queue_len == 0 {
        0.0
    } else if queue_len > QUEUE_PREVIEW_LIMIT && s.expanded.contains(QUEUE_ALL_KEY) {
        QUEUE_HEIGHT + S32
    } else {
        let rows = u16::try_from(queue_len.min(QUEUE_PREVIEW_LIMIT)).unwrap_or_default();
        f32::from(rows).mul_add(QUEUE_ROW_HEIGHT + HAIRLINE, S32)
    };
    BASE_CLEARANCE + queue_extra + tasks::clearance(s)
}

pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    if s.selected.is_none() && s.new_thread.is_none() {
        return None;
    }
    let mut column = col();
    if let Some(dock) = tasks::dock(s, t) {
        column = column.child(dock);
    }
    if let Some((id, state)) = s.selected.zip(s.state.as_ref())
        && !state.queue.is_empty()
    {
        column = column.child(queue(s, id, &state.queue, t));
    }
    if let Some(drawer) = completion::drawer(s, t) {
        column = column.child(drawer);
    }
    let editor = editor(s, t, layer);
    Some(ui::reading(column.child(editor)).key("composer").pb(S16))
}

fn on_draft_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if completion::on_key(s, ev, tx) {
        return true;
    }
    if ev.key == Key::Escape
        && !ev.mods.cmd
        && !ev.mods.ctrl
        && !ev.mods.alt
        && !ev.mods.shift
        && s.editing_queued.is_some()
    {
        s.cancel_edit_queued(tx);
        return true;
    }
    let primary_mod = if cfg!(target_os = "macos") {
        ev.mods.cmd && !ev.mods.ctrl
    } else {
        ev.mods.ctrl && !ev.mods.cmd
    };
    if s.editing_queued.is_none()
        && s.busy()
        && primary_mod
        && !ev.mods.shift
        && !ev.mods.alt
        && ev.key == Key::Enter
    {
        let flipped = match s.send_mode {
            SendMode::Queue => SendMode::Steer,
            SendMode::Steer => SendMode::Queue,
        };
        s.send_input_with_mode(Some(flipped), tx);
        return true;
    }
    false
}

fn editor(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let editing = s.editing_queued.is_some();
    let placeholder = if !s.connected() {
        "Offline"
    } else if editing {
        "Edit queued message…"
    } else if s.busy() {
        match s.send_mode {
            SendMode::Queue => "Queue a follow-up message…",
            SendMode::Steer => "Steer the active turn…",
        }
    } else {
        "Ask anything, / for commands, @ for files…"
    };
    let field = text_input(&s.draft, t.body)
        .key("draft")
        .label(if editing {
            "Edit queued message"
        } else {
            "Message"
        })
        .w_full()
        .color(t.text)
        .caret(t.accent, t.k.selection())
        .placeholder(placeholder, t.faint)
        .on_edit(|s: &mut Desktop| &mut s.draft)
        .on_input_paste(attachments::paste)
        .on_key(on_draft_key)
        .on_submit(Desktop::send_input);
    let attach = (!editing && (s.new_thread.is_some() || s.state.is_some()))
        .then(|| attachments::button(s, t));
    let show_mode_toggle = !editing
        && (s.busy()
            || s.state
                .as_ref()
                .is_some_and(|state| !state.queue.is_empty()));
    let mut card = col()
        .key("composer-card")
        .paint_order(30)
        .w_full()
        .radius(RADIUS_COMPOSER)
        .continuous_corners()
        .backdrop_blur(GLASS_BLUR)
        .fill(t.glass);
    if editing {
        card = card
            .border(HAIRLINE, t.accent.alpha(0.65))
            .border_dash(5.0, 3.0);
    } else {
        let (rim_top, rim_bottom) = if s.draft.focused {
            (t.rim_focus, t.rim_focus)
        } else {
            (t.rim_top, t.rim_bottom)
        };
        card = card.border_gradient(HAIRLINE, rim_top, rim_bottom);
    }
    if editing {
        card = card.child(edit_banner(s, t));
    }
    card = card
        .children((!editing).then(|| attachments::draft_chips(s, t)).flatten())
        .child(
            col()
                .w_full()
                .px(CARD_INSET)
                .pt(if editing { S8 } else { CARD_INSET })
                .pb(S4)
                .min_h(S48)
                .child(rune::auto_grow(field, 1, MAX_LINES)),
        )
        .child(
            row()
                .key("composer-toolbar")
                .w_full()
                .h(TOOLBAR_HEIGHT)
                .px(TOOLBAR_PAD_X)
                .pb(S8)
                .gap(TOOLBAR_GAP)
                .center_y()
                .instant_changes()
                .children(attach)
                .child(settings::toolbar(s, t, layer).grow(1.0).shrink().min_w(0.0))
                .children(show_mode_toggle.then(|| mode_toggle(s.send_mode, t)))
                .child(send_button(s, t)),
        );
    for shadow in t.shadow_composer {
        card = card.box_shadow(shadow);
    }
    if s.draft.focused {
        card = card.box_shadow(t.shadow_focus);
    }
    if !editing && s.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
}

/// Top micro-bar inside the composer card while editing a queued message.
fn edit_banner(s: &Desktop, t: &Theme) -> El {
    let index = s
        .editing_queued
        .as_ref()
        .zip(s.state.as_ref())
        .and_then(|(editing, state)| {
            state
                .queue
                .iter()
                .position(|item| item.run == editing.run)
                .map(|i| i + 1)
        });
    let label = index.map_or_else(
        || "Editing queued message".to_owned(),
        |n| format!("Editing queued #{n}"),
    );
    col()
        .key("edit-queued-banner")
        .w_full()
        .child(
            row()
                .w_full()
                .h(S24 + S4)
                .px(CARD_INSET)
                .gap(S6)
                .center_y()
                .child(
                    icon(&ui::PENCIL, ICON_SMALL)
                        .color(t.accent_text)
                        .a11y_hidden(),
                )
                .child(text(label).font(t.small_medium).color(t.text))
                .child(
                    text("· Esc cancel · ↵ save")
                        .font(t.small)
                        .color(t.faint)
                        .a11y_hidden(),
                )
                .child(rune::spacer())
                .child(
                    row()
                        .key("cancel-edit-queued")
                        .role("button")
                        .label("Cancel editing queued message")
                        .px(S4)
                        .h(S24)
                        .center_y()
                        .hover_group()
                        .press_scale(0.97)
                        .focusable(true)
                        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                        .on_press_tx(Desktop::cancel_edit_queued)
                        .child(
                            text("Cancel")
                                .font(t.small_medium)
                                .color(t.muted)
                                .group_color(t.text)
                                .a11y_hidden(),
                        ),
                ),
        )
        .child(ui::rule(t))
}

/// Inline ink toggle for switching the busy-turn delivery default between
/// Queue and Steer.
fn mode_toggle(mode: SendMode, t: &Theme) -> El {
    let (glyph, label, rest, hover, tip) = match mode {
        SendMode::Queue => (
            &ui::LIST_PLUS,
            "Queue",
            t.muted,
            t.text,
            "Default while busy: Queue (click for Steer)",
        ),
        SendMode::Steer => (
            &ui::CORNER_UP_RIGHT,
            "Steer",
            t.steer,
            t.steer,
            "Default while busy: Steer (click for Queue)",
        ),
    };
    row()
        .key("send-mode-toggle")
        .role("button")
        .label(label)
        .help(tip)
        .h(Size::Default.height())
        .px(S6)
        .gap(S4)
        .center_y()
        .radius(RADIUS_CONTROL)
        .continuous_corners()
        .hover_group()
        .press_scale(0.97)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(Desktop::toggle_send_mode)
        .child(
            icon(glyph, ICON_SMALL)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.small_medium)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
}

const QUEUE_ALL_KEY: &str = "queue-all";

fn queue(s: &Desktop, id: ThreadId, inputs: &[QueuedInput], t: &Theme) -> El {
    let total = inputs.len();
    let show_all = s.expanded.contains(QUEUE_ALL_KEY);
    let expanded = show_all || total <= QUEUE_PREVIEW_LIMIT;
    let visible = if expanded {
        inputs
    } else {
        &inputs[..QUEUE_PREVIEW_LIMIT]
    };
    let mut header = row()
        .key("queue-header")
        .w_full()
        .h(Size::Default.height())
        .px(S12)
        .gap(S6)
        .center_y()
        .child(icon(&ui::CLOCK, ICON_SMALL).color(t.faint).a11y_hidden())
        .child(
            text(format!("{total} queued"))
                .font(t.small_medium)
                .color(t.muted),
        );
    if total > QUEUE_PREVIEW_LIMIT {
        let toggle_label = if show_all {
            "Show fewer".to_owned()
        } else {
            format!("Show all {total}")
        };
        header = header.child(
            row()
                .key("queue-expand")
                .role("button")
                .label(&toggle_label)
                .px(S4)
                .h(S24)
                .center_y()
                .hover_group()
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .on_press(|s: &mut Desktop| s.toggle(QUEUE_ALL_KEY))
                .child(
                    text(toggle_label)
                        .font(t.small)
                        .color(t.faint)
                        .group_color(t.text)
                        .a11y_hidden(),
                ),
        );
    }
    header = header
        .child(rune::spacer())
        .child(queue_mode_radio(s.send_mode, t));

    let mut inner = col()
        .key("queue-rows")
        .w_full()
        .growth(GrowthSpec::default());
    for (index, input) in visible.iter().enumerate() {
        if index > 0 {
            inner = inner.child(
                el().w_full()
                    .h(HAIRLINE)
                    .fill(t.hairline.alpha(0.5))
                    .a11y_hidden(),
            );
        }
        inner = inner.child(queue_row(s, id, inputs, index, input, t));
    }
    let list = if expanded && total > QUEUE_PREVIEW_LIMIT {
        col()
            .key("queue-scroll")
            .w_full()
            .max_h(QUEUE_HEIGHT)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_edge_fade(S12)
            .child(inner)
    } else {
        inner
    };

    let deck_motion = Transition::offset(0.0, S12)
        .combined(Transition::opacity())
        .springs(SpringPair::new(
            Spring::new(0.20, 0.0),
            Spring::new(0.15, 0.0),
        ));
    ui::tucked_deck("queue", 20, t.dock_glass, ui::dock_blur(), t)
        .role("group")
        .label("Queued inputs")
        .transition(deck_motion)
        .child(header)
        .child(ui::rule(t))
        .child(list)
}

/// Compact `"Default: Queue / Steer"` ink radio switch in the queue header.
fn queue_mode_radio(current: SendMode, t: &Theme) -> El {
    let choice = |mode: SendMode, label: &'static str, active_color: Color| {
        let on = current == mode;
        row()
            .key(format!("queue-default-{label}"))
            .role("radio")
            .label(label)
            .checked(on)
            .px(S4)
            .h(S24)
            .center_y()
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press(move |s: &mut Desktop| s.send_mode = mode)
            .child(
                text(label)
                    .font(if on { t.small_medium } else { t.small })
                    .color(if on { active_color } else { t.faint })
                    .group_color(if on { active_color } else { t.muted })
                    .a11y_hidden(),
            )
    };
    row()
        .role("radiogroup")
        .label("Default delivery mode")
        .gap(S2)
        .center_y()
        .child(
            text("Default:")
                .font(t.small)
                .color(t.faint)
                .mr(S2)
                .a11y_hidden(),
        )
        .child(choice(SendMode::Queue, "Queue", t.text))
        .child(text("/").font(t.small).color(t.hairline).a11y_hidden())
        .child(choice(SendMode::Steer, "Steer", t.steer))
}

fn queue_row(
    s: &Desktop,
    thread: ThreadId,
    all: &[QueuedInput],
    index: usize,
    input: &QueuedInput,
    t: &Theme,
) -> El {
    let run = input.run;
    let is_editing = s
        .editing_queued
        .as_ref()
        .is_some_and(|editing| editing.thread == thread && editing.run == run);
    let title = input
        .text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Attachment");
    let actions = queue_row_actions(s, thread, all, index, input, is_editing, t);
    let mut row_el = row()
        .key(format!("queue-{run}"))
        .w_full()
        .h(QUEUE_ROW_HEIGHT)
        .px(TOOLBAR_PAD_X)
        .py(S4)
        .gap(S8)
        .center_y()
        .hover_group();
    if is_editing {
        row_el = row_el.child(
            el().w(S2)
                .h(S16)
                .radius(HAIRLINE)
                .fill(t.accent)
                .a11y_hidden(),
        );
    }
    row_el
        .child(queue_position(
            thread,
            run,
            index,
            all.len(),
            s.connected(),
            is_editing,
            t,
        ))
        .child(
            text(title)
                .font(t.ui)
                .color(if is_editing { t.text } else { t.muted })
                .group_color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .help(&input.text),
        )
        .children((input.attachment_count > 0).then(|| {
            row()
                .gap(S4)
                .center_y()
                .label(format!("{} attached files", input.attachment_count))
                .child(
                    icon(&ui::PAPERCLIP, ICON_SMALL)
                        .color(t.faint)
                        .a11y_hidden(),
                )
                .child(
                    text(input.attachment_count.to_string())
                        .font(t.mono_micro)
                        .color(t.faint),
                )
        }))
        .children(is_editing.then(|| {
            text("Editing")
                .font(t.mono_micro)
                .color(t.accent_text)
                .px(S6)
                .py(S2)
                .radius(RADIUS_INNER)
                .fill(t.accent_veil)
        }))
        .child(actions)
}

fn queue_position(
    thread: ThreadId,
    run: RunId,
    index: usize,
    length: usize,
    connected: bool,
    editing: bool,
    t: &Theme,
) -> El {
    let mark = text((index + 1).to_string())
        .font(t.mono_micro)
        .color(if editing { t.accent_text } else { t.faint })
        .a11y_hidden();
    let handle = el()
        .key("queue-position")
        .w(14.0)
        .h(S24)
        .center()
        .child(mark);
    if !connected || length < 2 {
        return handle;
    }
    handle
        .cursor(Cursor::Grab)
        .help("Drag to reorder")
        .on_drag(move |s: &mut Desktop, drag, tx| {
            if !matches!(drag.phase, Phase::Up) || drag.cancelled() || s.selected != Some(thread) {
                return;
            }
            let Some(queue) = s.state.as_ref().map(|state| &state.queue) else {
                return;
            };
            let Some(from) = queue.iter().position(|item| item.run == run) else {
                return;
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the drag is rounded to whole rows"
            )]
            let offset = (drag.dy / (QUEUE_ROW_HEIGHT + HAIRLINE)).round() as isize;
            let to = from.saturating_add_signed(offset).min(queue.len() - 1);
            if to != from {
                let before = queue.get(to + usize::from(to > from)).map(|item| item.run);
                s.send(
                    Command::MoveQueued {
                        thread,
                        run,
                        before,
                    },
                    tx,
                );
            }
        })
}

fn queue_row_actions(
    s: &Desktop,
    thread: ThreadId,
    all: &[QueuedInput],
    index: usize,
    input: &QueuedInput,
    is_editing: bool,
    t: &Theme,
) -> El {
    let run = input.run;
    let mut actions = row().gap(S2).center_y();
    if !s.connected() {
        return actions;
    }
    if index > 0 {
        let target = all[index - 1].run;
        actions = actions.child(
            queue_icon_action("up", &ui::ARROW_UP, "Move up", t.faint, t.text, t)
                .group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.send(
                        Command::MoveQueued {
                            thread,
                            run,
                            before: Some(target),
                        },
                        tx,
                    );
                }),
        );
    }
    if index + 1 < all.len() {
        let target = all.get(index + 2).map(|item| item.run);
        actions = actions.child(
            queue_icon_action("down", &ui::ARROW_DOWN, "Move down", t.faint, t.text, t)
                .group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.send(
                        Command::MoveQueued {
                            thread,
                            run,
                            before: target,
                        },
                        tx,
                    );
                }),
        );
    }
    let body = input.text.clone();
    actions = actions
        .child(
            queue_icon_action(
                "edit",
                &ui::PENCIL,
                "Edit queued message",
                t.faint,
                t.text,
                t,
            )
            .group_opacity(if is_editing { 1.0 } else { 0.0 }, 1.0)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.start_edit_queued(thread, run, &body, tx);
            }),
        )
        .child(
            queue_icon_action(
                "remove",
                &ui::CLOSE,
                "Remove queued message",
                t.faint,
                t.danger,
                t,
            )
            .group_opacity(0.0, 1.0)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.send(Command::RemoveQueued { thread, run }, tx);
            }),
        )
        .child(queue_send_now(thread, run, index == 0, t));
    actions
}

fn queue_send_now(thread: ThreadId, run: RunId, labeled: bool, t: &Theme) -> El {
    if labeled {
        row()
            .key("send-now")
            .role("button")
            .label("Send now")
            .help("Send next and stop the active turn, if any")
            .h(S24)
            .px(S6)
            .gap(S4)
            .center_y()
            .radius(RADIUS_INNER)
            .continuous_corners()
            .hover_group()
            .press_scale(0.97)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.send(Command::SteerQueued(SteerQueuedInput { thread, run }), tx);
            })
            .child(
                icon(&ui::CORNER_UP_RIGHT, ICON_SMALL)
                    .color(t.steer)
                    .a11y_hidden(),
            )
            .child(
                text("Send now")
                    .font(t.small_medium)
                    .color(t.steer)
                    .a11y_hidden(),
            )
    } else {
        queue_icon_action(
            "send-now",
            &ui::CORNER_UP_RIGHT,
            "Send now (steer active turn)",
            t.steer,
            t.steer,
            t,
        )
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.send(Command::SteerQueued(SteerQueuedInput { thread, run }), tx);
        })
    }
}

fn queue_icon_action(
    key: &'static str,
    glyph: &'static rune::Icon,
    label: &'static str,
    rest: Color,
    hover: Color,
    t: &Theme,
) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .size(S24, S24)
        .center()
        .radius(RADIUS_INNER)
        .continuous_corners()
        .hover_group()
        .press_scale(0.95)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .child(
            icon(glyph, ICON_SMALL)
                .color(rest)
                .group_color(hover)
                .a11y_hidden(),
        )
}

/// The composer's trailing circular action button(s): Check while editing a
/// queued message, Send when idle, a filled danger Stop circle while busy with
/// an empty draft, or a compact outline Stop ring beside the circular
/// Queue/Steer dispatch button when text is typed. Kept inside one stable
/// `composer-actions` wrapper with instant changes so a busy-to-idle thread
/// switch never leaves an exiting button beside the new one.
fn send_button(s: &Desktop, t: &Theme) -> El {
    let mut actions = row()
        .key("composer-actions")
        .gap(S6)
        .center_y()
        .instant_changes();
    if s.editing_queued.is_some() {
        let enabled = s.can_save_queued();
        let (fill, ink) = if enabled {
            (t.accent, t.on_accent)
        } else {
            (t.text.alpha(0.08), t.faint)
        };
        let mut save = circle_action(
            "send",
            "Save queued message",
            &ui::CHECK,
            fill,
            ink,
            enabled,
            t,
        );
        if enabled {
            save = save.on_press_tx(Desktop::save_edit_queued);
        }
        return actions.child(save);
    }
    if s.busy() {
        if s.can_send() {
            let stop_ring = row()
                .key("stop")
                .role("button")
                .label("Stop")
                .help(format!("Stop  {}", ui::shortcut(".")))
                .shortcut_hint(ui::shortcut("."))
                .size(SECONDARY_CIRCLE, SECONDARY_CIRCLE)
                .center()
                .radius(SECONDARY_CIRCLE / 2.0)
                .border(HAIRLINE, t.danger.alpha(0.45))
                .disabled(!s.connected())
                .focusable(s.connected())
                .focus_ring(HAIRLINE, S2, t.accent)
                .hover_group()
                .press_scale(0.93)
                .on_press_tx(Desktop::abort)
                .child(
                    el().size(STOP_GLYPH, STOP_GLYPH)
                        .radius(S2)
                        .fill(t.danger)
                        .a11y_hidden(),
                );
            let (glyph, label, fill) = match s.send_mode {
                SendMode::Queue => (&ui::LIST_PLUS, "Queue message", t.accent),
                SendMode::Steer => (&ui::CORNER_UP_RIGHT, "Steer active turn", t.steer),
            };
            let dispatch = circle_action("send", label, glyph, fill, t.on_accent, true, t)
                .on_press_tx(Desktop::send_input);
            actions = actions.child(stop_ring).child(dispatch);
        } else {
            let stop = row()
                .key("send")
                .role("button")
                .label("Stop")
                .help(format!("Stop  {}", ui::shortcut(".")))
                .shortcut_hint(ui::shortcut("."))
                .size(PRIMARY_CIRCLE, PRIMARY_CIRCLE)
                .center()
                .radius(PRIMARY_CIRCLE / 2.0)
                .fill(t.danger)
                .disabled(!s.connected())
                .focusable(s.connected())
                .focus_ring(HAIRLINE, S2, t.accent)
                .hover_group()
                .press_scale(0.93)
                .on_press_tx(Desktop::abort)
                .child(
                    el().size(STOP_GLYPH, STOP_GLYPH)
                        .radius(S2)
                        .fill(t.on_danger)
                        .a11y_hidden(),
                );
            actions = actions.child(stop);
        }
    } else {
        let enabled = s.can_send();
        let (fill, ink) = if enabled {
            (t.accent, t.on_accent)
        } else {
            (t.text.alpha(0.08), t.faint)
        };
        let mut send = circle_action("send", "Send", &ui::ARROW_UP, fill, ink, enabled, t);
        if enabled {
            send = send.on_press_tx(Desktop::send_input);
        }
        actions = actions.child(send);
    }
    actions
}

fn circle_action(
    key: &'static str,
    label: &'static str,
    glyph: &'static rune::Icon,
    fill: Color,
    ink: Color,
    enabled: bool,
    t: &Theme,
) -> El {
    let mut button = row()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .size(PRIMARY_CIRCLE, PRIMARY_CIRCLE)
        .center()
        .radius(PRIMARY_CIRCLE / 2.0)
        .fill(fill)
        .disabled(!enabled)
        .focusable(enabled)
        .focus_ring(HAIRLINE, S2, t.accent)
        .child(icon(glyph, ICON_SMALL + S2).color(ink).a11y_hidden());
    if enabled {
        button = button
            .linear_gradient(
                std::f32::consts::PI,
                [(0.0, t.on_accent.alpha(0.14)), (1.0, Color::TRANSPARENT)],
            )
            .hover_group()
            .press_scale(0.93);
    }
    button
}
