//! The composer: a floating card at the bottom of the reading column with a
//! growing field and rune-kit's round send button. Return sends and
//! Shift+Return breaks the line (Rune's multi-line `text_input`, with
//! `on_submit` on Return). rune-kit's `TextArea` brings its own outlined
//! field, so the card holds the bare core field instead.

use rune::{El, Tx, col, row, text_input};
use rune_kit::{Badge, BadgeVariant, IconButton, IconButtonVariant};

use crate::{
    app::Desktop,
    theme::{self, Theme},
};

/// Lines the field shows before it scrolls.
const MAX_LINES: usize = 10;
/// The send button's diameter.
const SEND: f32 = 30.0;

/// The composer under the timeline.
pub fn composer(s: &Desktop, t: &Theme) -> El {
    let k = &t.k;
    let placeholder = if s.connected() {
        "Message viz…"
    } else {
        "Waiting for the backend…"
    };
    let field = text_input(&s.draft, t.body)
        .key("draft")
        .label("Message")
        .w_full()
        .color(t.text)
        .caret(t.accent, k.selection())
        .placeholder(placeholder, t.faint)
        .on_edit(|s: &mut Desktop| &mut s.draft)
        // Return while disconnected keeps the text and says why in a toast.
        .on_submit(|s: &mut Desktop, tx: &mut Tx| {
            s.submit(tx);
            tx.scroll_to_bottom("timeline");
        });
    let send = IconButton::new(&theme::ARROW_UP)
        .variant(IconButtonVariant::Submit)
        .label("Send")
        .disabled(!s.can_send())
        .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
            s.submit(tx);
            tx.scroll_to_bottom("timeline");
        })
        .build(k)
        .key("send")
        .size(SEND, SEND)
        .radius(SEND / 2.0);
    let focused = s.draft.focused;
    let card = row()
        .key("composer-card")
        .w_full()
        .pl(16.0)
        .pr(8.0)
        .py(8.0)
        .gap(10.0)
        .items_end()
        .radius(k.radius.xl)
        .fill(t.surface)
        .border(
            1.0,
            if focused {
                k.c.border_strong
            } else {
                t.hairline
            },
        )
        .child(
            col()
                .key("field")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .py(3.5)
                .child(rune::auto_grow(field, 1, MAX_LINES)),
        )
        .child(send);
    let mut card = k.elevate(card, &k.elevation.floating);
    if focused {
        // The tide's glow, faintly, while the composer has focus.
        card = card.shadow(0.0, 18.0, t.glow.alpha(t.glow.a * 0.35));
    }
    let mut column = col()
        .key("composer-column")
        .w_full()
        .max_w(theme::READING_WIDTH)
        .gap(8.0);
    if let Some(queued) = queued(s) {
        column = column.child(
            Badge::new(queued)
                .variant(BadgeVariant::Subtle)
                .build(k)
                .key("queued")
                .ml(12.0),
        );
    }
    col()
        .key("composer")
        .w_full()
        .px(24.0)
        .pb(18.0)
        .items_center()
        .child(column.child(card))
}

/// How many inputs wait for the next turn, when any do.
fn queued(s: &Desktop) -> Option<String> {
    match s.thread.queued {
        0 => None,
        1 => Some("1 queued".to_owned()),
        n => Some(format!("{n} queued")),
    }
}
