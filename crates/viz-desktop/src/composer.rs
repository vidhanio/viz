//! Per-thread composer, settings, completions, and queued input.

mod completion;
mod draft;
mod model;
mod settings;

pub use completion::{Completion, poll};
pub use model::{ModelMenu, preview as preview_model_menu};
use rune::{El, ScrollSpec, col, row, text, text_input};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{QueuedInput, ThreadId};

use crate::{
    app::Desktop,
    attachments,
    server::Command,
    theme::{CARD_INSET, ICON, READING_INSET, S4, S8, S48, Size, Theme},
    ui::{self, Card},
};

const MAX_LINES: usize = 8;
/// The composer toolbar's inset: its buttons' glyphs line up with the text
/// above them.
const TOOLBAR_INSET: f32 = CARD_INSET - (Size::Default.height() - ICON) / 2.0;
/// The gap between the composer toolbar's items.
const TOOLBAR_GAP: f32 = S4;
/// The tallest the queue gets before it scrolls.
const QUEUE_HEIGHT: f32 = 136.0;

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
    Some(
        ui::reading(column.child(editor))
            .key("composer")
            .pb(READING_INSET),
    )
}

fn editor(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let placeholder = if !s.connected() {
        "Offline"
    } else if s.busy() {
        "Queue a message"
    } else {
        "Ask anything"
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
        .on_key(completion::on_key)
        .on_submit(Desktop::send_input);
    let attach = (s.new_thread.is_some() || s.state.is_some()).then(|| attachments::button(s, t));
    let card = Card::Surface
        .frame(t)
        .key("composer-card")
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
                .px(TOOLBAR_INSET)
                .pb(TOOLBAR_INSET)
                .gap(TOOLBAR_GAP)
                .center_y()
                .children(attach)
                .child(settings::toolbar(s, t, layer).grow(1.0).shrink().min_w(0.0))
                .child(send_button(s, t)),
        );
    if s.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
}

fn queue(s: &Desktop, id: ThreadId, inputs: &[QueuedInput], t: &Theme) -> El {
    Card::Surface
        .frame(t)
        .key("queue")
        .role("group")
        .label("Queued inputs")
        .max_h(QUEUE_HEIGHT)
        .pl(CARD_INSET)
        .pr(TOOLBAR_INSET)
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

/// The composer's trailing action(s): Send when idle, or an outline Stop button
/// while busy (with Queue beside it when the draft has something to send).
fn send_button(s: &Desktop, t: &Theme) -> El {
    if s.busy() {
        let stop = ui::icon_button(&ui::STOP, "Stop", IconButtonVariant::Neutral, Size::Default)
            .outline()
            .shortcut(ui::shortcut("."))
            .disabled(!s.connected())
            .on_press_tx(Desktop::abort)
            .build(&t.k)
            .key("stop");
        if s.can_send() {
            let queue = ui::icon_button(
                &ui::ARROW_UP,
                "Queue",
                IconButtonVariant::Submit,
                Size::Default,
            )
            .on_press_tx(Desktop::send_input)
            .build(&t.k)
            .key("send");
            row()
                .key("composer-actions")
                .gap(TOOLBAR_GAP)
                .center_y()
                .child(stop)
                .child(queue)
        } else {
            stop
        }
    } else {
        ui::icon_button(
            &ui::ARROW_UP,
            "Send",
            IconButtonVariant::Submit,
            Size::Default,
        )
        .disabled(!s.can_send())
        .on_press_tx(Desktop::send_input)
        .build(&t.k)
        .key("send")
    }
}
