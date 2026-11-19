//! Per-conversation composer, settings, completions, and pending approvals.

mod completion;
mod settings;

pub use completion::{Completion, poll};
use rune::{El, ScrollSpec, col, icon, row, text, text_input};
use rune_kit::{ButtonVariant, Collapsible, IconButtonVariant};
use viz_client::protocol::{Approval, ApprovalKind, ConversationId, QueuedInput};

use crate::{
    app::Desktop,
    attachments,
    backend::Command,
    theme::{self, Theme},
    ui,
};

const MAX_LINES: usize = 8;

pub fn composer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    if s.selected.is_none() && s.new_chat.is_none() {
        return None;
    }
    let mut column = col().w_full().max_w(theme::READING_WIDTH).gap(8.0);
    if let Some((id, state)) = s.selected.zip(s.conversation.as_ref())
        && !state.approvals.is_empty()
    {
        column = column.child(
            col()
                .key("approvals")
                .w_full()
                .max_h(260.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .gap(6.0)
                .children(
                    state
                        .approvals
                        .iter()
                        .map(|approval| approval_card(s, id, approval, t)),
                ),
        );
    }
    if let Some((id, state)) = s.selected.zip(s.conversation.as_ref())
        && !state.queue.is_empty()
    {
        column = column.child(queue(s, id, &state.queue, t));
    }
    let editor = if s.conversation.as_ref().is_some_and(|state| state.read_only) {
        let mut notice = row()
            .key("read-only")
            .w_full()
            .center_y()
            .gap(8.0)
            .px(12.0)
            .py(9.0)
            .radius(t.k.radius.md)
            .border(1.0, t.hairline)
            .fill(t.surface)
            .child(icon(t.k.glyph.lock, 13.0).color(t.muted))
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
        col()
            .key("composer")
            .w_full()
            .px(22.0)
            .pb(18.0)
            .items_center()
            .child(column.child(editor)),
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
        .on_submit(Desktop::submit);
    let mut metadata = row().key("composer-metadata").gap(6.0).center_y();
    if let Some(branch) = s.branch() {
        metadata = metadata.child(
            row()
                .key("branch")
                .center_y()
                .gap(4.0)
                .max_w(115.0)
                .help(branch)
                .child(icon(&ui::GIT_BRANCH, 13.0).color(t.muted))
                .child(
                    text(branch)
                        .font(t.small)
                        .color(t.muted)
                        .truncate()
                        .shrink(),
                ),
        );
    }
    let card = col()
        .key("composer-card")
        .w_full()
        .radius(t.k.radius.lg)
        .fill(t.surface)
        .border(1.0, t.hairline)
        .children(attachments::draft_chips(s, t))
        .child(
            col()
                .w_full()
                .px(13.0)
                .pt(10.0)
                .pb(6.0)
                .min_h(48.0)
                .child(rune::auto_grow(field, 1, MAX_LINES)),
        )
        .child(
            row()
                .key("composer-toolbar")
                .w_full()
                .px(7.0)
                .pb(7.0)
                .gap(8.0)
                .items_end()
                .children(s.conversation.as_ref().map(|_| attachments::button(s, t)))
                .child(settings::toolbar(s, t, layer).grow(1.0).shrink().min_w(0.0))
                .child(metadata.mb(4.0))
                .child(send_button(s, t)),
        );
    if s.can_attach() {
        card.on_drop_files(attachments::drop_files)
    } else {
        card
    }
}

fn queue(s: &Desktop, id: ConversationId, inputs: &[QueuedInput], t: &Theme) -> El {
    col()
        .key("queue")
        .role("group")
        .label("Queued inputs")
        .w_full()
        .max_h(134.0)
        .padding(7.0)
        .gap(2.0)
        .fill(t.surface)
        .radius(t.k.radius.md)
        .border(1.0, t.hairline)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(ui::section("Queued", t).px(5.0))
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
                .h(25.0)
                .center_y()
                .gap(5.0)
                .px(5.0)
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
                        t.k.glyph.close,
                        "Remove queued input",
                        IconButtonVariant::GhostMuted,
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
                    .key("remove")
                    .size(20.0, 20.0),
                )
        }))
}

fn send_button(s: &Desktop, t: &Theme) -> El {
    if s.busy() {
        ui::icon_button(&ui::STOP, "Stop", IconButtonVariant::Neutral)
            .shortcut(format!("{}+.", ui::COMMAND_KEY))
            .disabled(!s.connected())
            .on_press_tx(Desktop::abort)
            .build(&t.k)
            .key("stop")
            .size(29.0, 29.0)
    } else {
        ui::icon_button(&theme::ARROW_UP, "Send", IconButtonVariant::Submit)
            .disabled(!s.can_send())
            .on_press_tx(Desktop::submit)
            .build(&t.k)
            .key("send")
            .size(29.0, 29.0)
    }
}

fn approval_card(s: &Desktop, conversation: ConversationId, approval: &Approval, t: &Theme) -> El {
    let key = format!("approval-{}", approval.id);
    let mut card = col()
        .key(key.clone())
        .w_full()
        .padding(11.0)
        .gap(8.0)
        .radius(t.k.radius.md)
        .fill(t.surface)
        .border(1.0, t.hairline)
        .child(
            row()
                .key("request")
                .gap(7.0)
                .center_y()
                .child(icon(t.k.glyph.lock, 13.0).color(t.warning))
                .child(ui::section("Permission needed", t).grow(1.0))
                .children(approval.kind.as_ref().map(|kind| ui::chip(kind, t))),
        )
        .child(
            text(&approval.title)
                .key("title")
                .font(t.ui_medium)
                .color(t.text)
                .wrap(),
        );
    if let Some(detail) = &approval.detail {
        let open = s.expanded.contains(&key);
        card = card.child(
            Collapsible::new("Details", open)
                .ghost()
                .on_toggle(move |s: &mut Desktop| s.toggle(&key))
                .child(
                    col()
                        .key("approval-detail")
                        .w_full()
                        .max_h(120.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec {
                            stick_bottom: false,
                        })
                        .padding(7.0)
                        .child(ui::code(detail, t)),
                )
                .build(&t.k),
        );
    }
    card.child(
        row()
            .key("responses")
            .w_full()
            .gap(6.0)
            .flex_wrap()
            .justify_end()
            .children(approval.options.iter().map(|option| {
                let variant = match option.kind {
                    ApprovalKind::AllowOnce => ButtonVariant::Submit,
                    ApprovalKind::AllowAlways => ButtonVariant::Outline,
                    ApprovalKind::RejectOnce | ApprovalKind::RejectAlways => ButtonVariant::Danger,
                };
                let approval_id = approval.id.clone();
                let option_id = option.id.clone();
                ui::button(&option.name, variant)
                    .disabled(!s.connected())
                    .on_press_tx(move |s: &mut Desktop, tx| {
                        s.send(
                            Command::Approval {
                                conversation,
                                approval: approval_id.clone(),
                                option: option_id.clone(),
                            },
                            tx,
                        );
                    })
                    .build(&t.k)
                    .key(&option.id)
            })),
    )
}
