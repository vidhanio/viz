//! The composer: a card with a growing multi-line field, the conversation's
//! model and thinking chips, key hints and the send button. Return sends and
//! Shift+Return (or Option+Return) breaks the line: that's Rune's multi-line
//! `text_input` behaviour, with `on_submit` on Return.

use rune::{El, TextStyle, Tx, col, row, text, text_input};
use rune_kit::{Badge, BadgeVariant, IconButton, IconButtonVariant, Keybind, KeybindVariant, Kit};
use rune_theme_opencode::icons;

use crate::{app::Desktop, header, theme};

/// Lines the field shows before it scrolls.
const MAX_LINES: usize = 10;

/// The composer under the timeline.
pub fn composer(s: &Desktop, k: &Kit) -> El {
    let c = &k.c;
    let connected = s.connected();
    let placeholder = match s.selected {
        _ if !connected => "Waiting for the backend…".to_owned(),
        Some(id) if Some(id) == s.root => "Message the orchestrator".to_owned(),
        Some(id) => format!("Message {}", s.title(id)),
        None => "Message".to_owned(),
    };
    let style = TextStyle {
        line_height: 21.0,
        ..k.ty.body
    };
    let field = text_input(&s.draft, style)
        .key("draft")
        .label("Message")
        .w_full()
        .color(c.text_base)
        .caret(c.text_base, k.selection())
        .placeholder(placeholder, c.text_faint)
        .on_edit(|s: &mut Desktop| &mut s.draft)
        // Return while disconnected keeps the text and says why in a toast.
        .on_submit(|s: &mut Desktop, tx: &mut Tx| {
            s.submit(tx);
            tx.scroll_to_bottom("timeline");
        });
    let footer = footer(s, k);
    let focused = s.draft.focused;
    let card = col()
        .key("composer-card")
        .w_full()
        .px(12.0)
        .pt(10.0)
        .pb(8.0)
        .gap(8.0)
        .radius(k.radius.xl)
        .fill(c.layer_01)
        .border(
            1.0,
            if focused {
                c.border_strong
            } else {
                c.border_base
            },
        )
        .child(rune::auto_grow(field, 1, MAX_LINES))
        .child(footer);
    let mut column = col()
        .key("composer-column")
        .w_full()
        .max_w(theme::READING_WIDTH)
        .gap(6.0);
    if let Some(status) = status_line(s) {
        column = column.child(
            row()
                .key("composer-status")
                .px(4.0)
                .gap(6.0)
                .center_y()
                .child(theme::dot(c.warning.icon, 6.0))
                .child(text(status).font(k.ty.small).color(c.text_muted)),
        );
    }
    col()
        .key("composer")
        .w_full()
        .px(24.0)
        .pb(16.0)
        .items_center()
        .child(column.child(k.elevate(card, &k.elevation.raised)))
}

/// What the composer should say above the card: queued inputs, or that a
/// message sent now waits for the current run.
fn status_line(s: &Desktop) -> Option<String> {
    let thread = &s.thread;
    match (thread.queued, thread.live.busy) {
        (0, false) => None,
        (0, true) => Some("Working: a message sent now runs once this turn ends".into()),
        (1, _) => Some("1 message queued for the next turn".into()),
        (n, _) => Some(format!("{n} messages queued for the next turn")),
    }
}

/// The card's bottom row: model and thinking chips, key hints, send.
fn footer(s: &Desktop, k: &Kit) -> El {
    let c = &k.c;
    let send = IconButton::new(&icons::ARROW_UP)
        .variant(IconButtonVariant::Submit)
        .label("Send")
        .help("Send")
        .shortcut("⏎")
        .disabled(!s.can_send())
        .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
            s.submit(tx);
            tx.scroll_to_bottom("timeline");
        })
        .build(k)
        .key("send");
    let mut chips = row().key("chips").gap(6.0).center_y().grow(1.0).shrink();
    let agent = &s.thread.agent;
    if let Some(model) = &agent.model {
        chips = chips.child(header::model_chip(agent.provider.as_deref(), model, k));
    }
    if let Some(level) = &agent.thinking {
        chips = chips.child(
            Badge::new(format!("thinking {level}"))
                .variant(BadgeVariant::Subtle)
                .build(k)
                .key("thinking"),
        );
    }
    let hint = |keys: &[&str], what: &str| {
        row()
            .gap(4.0)
            .center_y()
            .child(Keybind::new(keys).variant(KeybindVariant::Ghost).build(k))
            .child(text(what.to_owned()).font(k.ty.small).color(c.text_faint))
    };
    row()
        .key("composer-footer")
        .w_full()
        .gap(10.0)
        .center_y()
        .child(chips)
        .child(hint(&["⏎"], "send"))
        .child(hint(&["⇧", "⏎"], "newline"))
        .child(send)
}
