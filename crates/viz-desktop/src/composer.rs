//! Per-thread composer, settings, completions, and queued input.

mod completion;
mod draft;
mod model;
mod settings;

pub use completion::{Completion, poll};
pub use model::{ModelMenu, preview as preview_model_menu};
use rune::{Color, El, Key, KeyEvent, ScrollSpec, Tx, col, el, icon, row, text, text_input};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{QueuedInput, SendMode, ThreadId};

use crate::{
    app::Desktop,
    attachments,
    server::Command,
    theme::{
        CARD_INSET, GLASS_BLUR, HAIRLINE, ICON_SMALL, RADIUS_COMPOSER, RADIUS_CONTROL, S2, S4, S6,
        S8, S16, S24, S48, Size, Theme,
    },
    ui::{self, Card},
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
/// The tallest the queue gets before it scrolls.
const QUEUE_HEIGHT: f32 = 136.0;
/// Bottom clearance reserved in the scrolling transcript when the composer is
/// mounted.
const BASE_CLEARANCE: f32 = 148.0;
/// Extra clearance per queued row above the composer.
const QUEUE_ROW_CLEARANCE: f32 = 34.0;

/// Bottom padding the transcript needs so its latest message scrolls completely
/// clear of the floating composer stack.
pub fn clearance(s: &Desktop) -> f32 {
    if s.selected.is_none() && s.new_thread.is_none() {
        return S24;
    }
    let queue_rows =
        u16::try_from(s.state.as_ref().map_or(0, |state| state.queue.len().min(4))).unwrap_or(4);
    let queue_extra = if queue_rows > 0 {
        f32::from(queue_rows).mul_add(QUEUE_ROW_CLEARANCE, 32.0)
    } else {
        0.0
    };
    BASE_CLEARANCE + queue_extra
}

pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    if s.selected.is_none() && s.new_thread.is_none() {
        return None;
    }
    let mut column = col().gap(S8);
    if let Some((id, state)) = s.selected.zip(s.state.as_ref())
        && !state.queue.is_empty()
    {
        column = column.child(queue(s, id, &state.queue, t));
    }
    completion::popup(s, t, layer);
    let editor = editor(s, t, layer);
    Some(ui::reading(column.child(editor)).key("composer").pb(S16))
}

fn on_draft_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if completion::on_key(s, ev, tx) {
        return true;
    }
    let primary_mod = if cfg!(target_os = "macos") {
        ev.mods.cmd && !ev.mods.ctrl
    } else {
        ev.mods.ctrl && !ev.mods.cmd
    };
    if s.busy() && primary_mod && !ev.mods.shift && !ev.mods.alt && ev.key == Key::Enter {
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
    let placeholder = if !s.connected() {
        "Offline"
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
        .label("Message")
        .w_full()
        .color(t.text)
        .caret(t.accent, t.k.selection())
        .placeholder(placeholder, t.faint)
        .on_edit(|s: &mut Desktop| &mut s.draft)
        .on_input_paste(attachments::paste)
        .on_key(on_draft_key)
        .on_submit(Desktop::send_input);
    let attach = (s.new_thread.is_some() || s.state.is_some()).then(|| attachments::button(s, t));
    let show_mode_toggle = s.busy()
        || s.state
            .as_ref()
            .is_some_and(|state| !state.queue.is_empty());
    let (rim_top, rim_bottom) = if s.draft.focused {
        (t.rim_focus, t.rim_focus)
    } else {
        (t.rim_top, t.rim_bottom)
    };
    let mut card = col()
        .key("composer-card")
        .paint_order(30)
        .w_full()
        .radius(RADIUS_COMPOSER)
        .continuous_corners()
        .backdrop_blur(GLASS_BLUR)
        .fill(t.glass)
        .border_gradient(HAIRLINE, rim_top, rim_bottom)
        .children(attachments::draft_chips(s, t))
        .child(
            col()
                .w_full()
                .px(CARD_INSET)
                .pt(CARD_INSET)
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
    if s.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
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

fn queue(s: &Desktop, id: ThreadId, inputs: &[QueuedInput], t: &Theme) -> El {
    Card::Surface
        .frame(t)
        .key("queue")
        .role("group")
        .label("Queued inputs")
        .max_h(QUEUE_HEIGHT)
        .pl(CARD_INSET)
        .pr(TOOLBAR_PAD_X)
        .py(S8)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            row()
                .h(Size::Small.height())
                .center_y()
                .child(ui::section("Queued", t)),
        )
        .children(inputs.iter().map(|input| {
            let run = input.run;
            let title = input
                .text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Attachment");
            row()
                .key(format!("queue-{run}"))
                .w_full()
                .h(Size::Small.height())
                .center_y()
                .gap(S8)
                .child(
                    text(title)
                        .font(t.ui)
                        .color(t.muted)
                        .grow(1.0)
                        .shrink()
                        .truncate()
                        .help(&input.text),
                )
                .child(
                    ui::icon_button(
                        &ui::CLOSE,
                        "Remove queued input",
                        IconButtonVariant::GhostMuted,
                        Size::Small,
                    )
                    .disabled(!s.connected())
                    .on_press_tx(move |s: &mut Desktop, tx| {
                        s.send(Command::RemoveQueued { thread: id, run }, tx);
                    })
                    .build(&t.k)
                    .key("remove"),
                )
        }))
}

/// The composer's trailing circular action button(s): Send when idle, a filled
/// danger Stop circle while busy with an empty draft, or a compact outline Stop
/// ring beside the circular Queue/Steer dispatch button when text is typed.
/// Kept inside one stable `composer-actions` wrapper with instant changes so a
/// busy-to-idle thread switch never leaves an exiting button beside the new
/// one.
fn send_button(s: &Desktop, t: &Theme) -> El {
    let mut actions = row()
        .key("composer-actions")
        .gap(S6)
        .center_y()
        .instant_changes();
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
