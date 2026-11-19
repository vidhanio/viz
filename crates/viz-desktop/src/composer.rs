//! Per-conversation composer, settings, completions, and queued input.

mod completion;
mod settings;
mod workspace;

pub use completion::{Completion, poll};
use rune::{El, ScrollSpec, col, icon, row, text, text_input};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{ConversationId, QueuedInput};

use crate::{
    app::Desktop,
    attachments,
    backend::Command,
    theme::{
        CARD_INSET, HAIRLINE, ICON, ICON_SMALL, READING_INSET, READING_WIDTH, S4, S8, S48, Size,
        Theme,
    },
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
    if s.selected.is_none() && s.new_chat.is_none() {
        return None;
    }
    let mut column = col().gap(S8);
    if let Some((id, state)) = s.selected.zip(s.conversation.as_ref())
        && !state.queue.is_empty()
    {
        column = column.child(queue(s, id, &state.queue, t));
    }
    let editor = if s.conversation.as_ref().is_some_and(|state| state.read_only) {
        let mut notice = Card::Surface
            .row(t)
            .key("read-only")
            .center_y()
            .gap(S8)
            .pl(CARD_INSET)
            .pr(TOOLBAR_INSET)
            .py(TOOLBAR_INSET)
            .child(ui::lead(
                icon(&ui::LOCK, ICON_SMALL).color(t.muted),
                Size::Default.height(),
            ))
            .child(
                text("Subagent is running")
                    .font(t.ui)
                    .color(t.muted)
                    .grow(1.0),
            );
        if s.busy() {
            notice = notice.child(send_button(s, t));
        }
        notice
    } else {
        completion::popup(s, t, layer);
        editor(s, t, layer)
    };
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
    } else if s.selected.is_some() && s.selected == s.root {
        "Message the orchestrator"
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
        .on_submit(Desktop::submit);
    let locked = workspace::locked(s);
    let metadata = locked.map(|(label, path)| ui::locked_label("workspace-locked", label, path, t));
    let attach =
        (s.new_chat.is_some() || s.conversation.is_some()).then(|| attachments::button(s, t));
    let budget = settings_budget(s, attach.is_some(), locked.map(|(label, _)| label), t);
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
                .child(
                    settings::toolbar(s, t, layer, budget)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0),
                )
                .children(metadata)
                .child(send_button(s, t)),
        );
    if s.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
}

/// Points the settings selects may take in one toolbar row: the main pane
/// between the open panes, less the composer's margins and the toolbar's other
/// items.
fn settings_budget(s: &Desktop, attach: bool, locked: Option<&str>, t: &Theme) -> f32 {
    let button = Size::Default.height();
    let main = s.window_width - s.sidebar.shown_width() - s.changes.pane.shown_width();
    // Both sides: the reading inset, then the card's border and toolbar inset.
    let card = 2.0f32.mul_add(
        -(HAIRLINE + TOOLBAR_INSET),
        2.0f32.mul_add(-READING_INSET, main).min(READING_WIDTH),
    );
    let workspace = locked.map_or(0.0, |label| ui::locked_label_width(label, t));
    let send = TOOLBAR_GAP.mul_add(2.0, button);
    let fixed = if attach {
        send + button + TOOLBAR_GAP
    } else {
        send
    };
    card - fixed - workspace
}

fn queue(s: &Desktop, id: ConversationId, inputs: &[QueuedInput], t: &Theme) -> El {
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
            let submission = input.submission;
            let title = input
                .text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Attachment");
            row()
                .key(format!("queue-{submission}"))
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
                        s.send(
                            Command::RemoveQueued {
                                conversation: id,
                                submission,
                            },
                            tx,
                        );
                    })
                    .build(&t.k)
                    .key("remove"),
                )
        }))
}

fn send_button(s: &Desktop, t: &Theme) -> El {
    if s.busy() {
        ui::icon_button(&ui::STOP, "Stop", IconButtonVariant::Neutral, Size::Default)
            .shortcut(format!("{}+.", ui::COMMAND_KEY))
            .disabled(!s.connected())
            .on_press_tx(Desktop::abort)
            .build(&t.k)
            .key("stop")
    } else {
        ui::icon_button(
            &ui::ARROW_UP,
            "Send",
            IconButtonVariant::Submit,
            Size::Default,
        )
        .disabled(!s.can_send())
        .on_press_tx(Desktop::submit)
        .build(&t.k)
        .key("send")
    }
}
