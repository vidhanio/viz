//! Per-thread composer, settings, and completions.

mod completion;
mod draft;
mod model;
mod settings;
mod tasks;

use rune::{Color, El, KeyEvent, Tx, col, el, icon, row, text_input};
pub use tasks::DOCK_KEY as TASKS_DOCK_KEY;

use crate::{
    app::Desktop,
    attachments,
    theme::{
        CARD_INSET, HAIRLINE, ICON_SMALL, RADIUS_CARD, READING_INSET, S2, S4, S6, S8, S48, Theme,
    },
    ui,
    update::{ComposerMsg, Msg},
};

const MAX_LINES: usize = 8;
/// Horizontal padding inside the composer's bottom controls bar.
const TOOLBAR_PAD_X: f32 = 10.0;
/// Minimum height of the composer's bottom controls bar.
const TOOLBAR_HEIGHT: f32 = 38.0;
/// Diameter of the primary circular action button.
const PRIMARY_CIRCLE: f32 = 30.0;
/// Diameter of the secondary circular Stop button beside Steer.
const SECONDARY_CIRCLE: f32 = 28.0;
/// Side length of the rounded square stop glyph inside the circular button.
const STOP_GLYPH: f32 = 9.0;
/// The gap between the composer toolbar's items.
const TOOLBAR_GAP: f32 = S4;

/// Renders the composer stack (Tasks Dock, completion drawer, editor card)
/// when a thread or fresh draft is active.
pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    if m.selection.selected.is_none() && m.selection.new_thread.is_none() {
        return None;
    }
    let mut column = col();
    if let Some(dock) = tasks::dock(s, t) {
        column = column.child(dock);
    }
    if let Some(drawer) = completion::drawer(s, t) {
        column = column.child(drawer);
    }
    let editor = editor(s, t, layer);
    Some(
        ui::reading(column.child(editor))
            .key("composer")
            .pb(READING_INSET),
    )
}

fn on_draft_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    completion::on_key(s, ev, tx)
}

fn editor(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let placeholder = if !m.connected() {
        "Offline"
    } else if m.busy() {
        "Steer the active turn…"
    } else {
        "Ask anything"
    };
    let field = text_input(&s.widgets.draft, t.body)
        .key("draft")
        .label("Message")
        .w_full()
        .color(t.text)
        .caret(t.accent, t.k.selection())
        .placeholder(placeholder, t.faint)
        .on_edit(|s: &mut Desktop| &mut s.widgets.draft)
        .on_input_change(|s: &mut Desktop, tx| {
            let text = s.widgets.draft.text.clone();
            s.dispatch(Msg::Composer(ComposerMsg::DraftChanged(text)), tx);
        })
        .on_input_paste(attachments::paste)
        .on_input_key(on_draft_key)
        .on_submit(|s: &mut Desktop, tx| s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx));
    let attach = (m.selection.new_thread.is_some() || m.thread_state().is_some())
        .then(|| attachments::button(s, t));
    let border = if s.widgets.draft.focused {
        t.k.c.border_strong
    } else {
        t.hairline
    };
    let card = col()
        .key("composer-card")
        .paint_order(30)
        .w_full()
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, border)
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
                .child(send_button(s, t)),
        );
    if m.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
}

/// The composer's trailing circular action button(s): Send when idle, a filled
/// danger Stop circle while busy with an empty draft, or a filled neutral Stop
/// circle beside the Steer button when text is typed. Kept inside one stable
/// `composer-actions` wrapper with instant changes so a busy-to-idle thread
/// switch never leaves an exiting button beside the new one.
fn send_button(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let actions = row()
        .key("composer-actions")
        .gap(S6)
        .center_y()
        .instant_changes();
    if m.busy() {
        return busy_actions(actions, s, t);
    }
    let enabled = m.can_send();
    let (fill, ink) = if enabled {
        (t.accent, t.on_accent)
    } else {
        (t.k.c.bg_button_neutral, t.faint)
    };
    let mut send = circle_action("send", "Send", &ui::ARROW_UP, fill, ink, enabled, t);
    if enabled {
        send = send.on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx);
        });
    }
    actions.child(send)
}

fn busy_actions(actions: El, s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    if !m.can_send() {
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
            .disabled(!m.connected())
            .focusable(m.connected())
            .focus_ring(HAIRLINE, S2, t.accent)
            .hover_group()
            .press_scale(0.93)
            .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Composer(ComposerMsg::Abort), tx))
            .child(
                el().size(STOP_GLYPH, STOP_GLYPH)
                    .radius(S2)
                    .fill(t.on_danger)
                    .a11y_hidden(),
            );
        return actions.child(stop);
    }
    let stop = row()
        .key("stop")
        .role("button")
        .label("Stop")
        .help(format!("Stop  {}", ui::shortcut(".")))
        .shortcut_hint(ui::shortcut("."))
        .size(SECONDARY_CIRCLE, SECONDARY_CIRCLE)
        .center()
        .radius(SECONDARY_CIRCLE / 2.0)
        .fill(t.k.c.bg_button_neutral)
        .disabled(!m.connected())
        .focusable(m.connected())
        .focus_ring(HAIRLINE, S2, t.accent)
        .hover_group()
        .press_scale(0.93)
        .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Composer(ComposerMsg::Abort), tx))
        .child(
            el().size(STOP_GLYPH, STOP_GLYPH)
                .radius(S2)
                .fill(t.danger)
                .a11y_hidden(),
        );
    let steer = circle_action(
        "send",
        "Steer active turn",
        &ui::CORNER_UP_RIGHT,
        t.accent,
        t.on_accent,
        true,
        t,
    )
    .help("Steer at the next safe step")
    .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx));
    actions.child(stop).child(steer)
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
        button = button.hover_group().press_scale(0.93);
    }
    button
}
