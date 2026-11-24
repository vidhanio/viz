//! Right-aligned dashed queued-messages stack rendered directly above the
//! composer dock while inputs wait for their turn.

use rune::{Color, El, col, el, row, text};
use rune_kit::IconButtonVariant;
use viz_client::protocol::QueuedInput;

use crate::{
    app::Desktop,
    motion,
    theme::{HAIRLINE, RADIUS_SURFACE, S2, S4, S8, S12, Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Maximum bubble width fraction matching user message bubbles (`80%`).
const BUBBLE_FRAC: f32 = 0.80;
/// Maximum queued cards shown before offering the overflow fold pill.
const VISIBLE_DEFAULT: usize = 3;

/// Renders the right-aligned dashed stack of queued messages when the active
/// thread has one or more queued inputs.
pub(super) fn stack(s: &Desktop, t: &Theme) -> Option<El> {
    let queue = s.model.thread_state().map(|state| state.queue.as_slice())?;
    if queue.is_empty() {
        return None;
    }
    let expanded = s.model.composer.queued_expanded;
    let total = queue.len();
    let visible = if expanded || total <= VISIBLE_DEFAULT {
        queue
    } else {
        &queue[..VISIBLE_DEFAULT]
    };
    let mut list = motion::reflow_container(
        col()
            .key("queued-stack")
            .role("list")
            .label("Queued messages")
            .w_full()
            .pb(S8)
            .gap(S8),
    );
    for item in visible {
        list = list.child(queued_card(s, item, t));
    }
    if total > VISIBLE_DEFAULT {
        let label = if expanded {
            "Show fewer".to_owned()
        } else {
            format!("Show all {total} queued")
        };
        list = list.child(
            row().key("queued-overflow").w_full().justify_end().child(
                rune_kit::Button::new(label)
                    .variant(rune_kit::ButtonVariant::GhostMuted)
                    .control_size(crate::theme::Size::Micro.kit())
                    .on_press_tx(|s: &mut Desktop, tx| {
                        s.dispatch(Msg::Composer(ComposerMsg::ToggleQueuedExpanded), tx);
                    })
                    .build(&t.k),
            ),
        );
    }
    Some(list)
}

fn queued_card(s: &Desktop, item: &QueuedInput, t: &Theme) -> El {
    let run = item.run;
    let editing = s.model.composer.editing_queued == Some(run);
    let connected = s.model.connected();
    let display = item.prompt.display_text();
    let attachment_count = item.prompt.attachments().count();

    let mut bubble = col()
        .key(format!("queued-bubble-{}", run.0))
        .role("listitem")
        .label(format!("Queued: {display}"))
        .w_full()
        .px(S12)
        .py(S8)
        .gap(S4)
        .radius(RADIUS_SURFACE)
        .fill(Color::TRANSPARENT);
    if editing {
        bubble = bubble.fill(t.hover_wash);
    } else {
        bubble = bubble
            .border(HAIRLINE, t.border_dashed)
            .border_dash(4.0, 3.0);
    }

    if attachment_count > 0 || editing {
        let mut meta = row().gap(S4).center_y();
        if editing {
            meta = meta.child(
                rune_kit::Badge::new("Editing")
                    .variant(rune_kit::BadgeVariant::Accent)
                    .build(&t.k),
            );
        }
        if attachment_count > 0 {
            let label = if attachment_count == 1 {
                "1 attachment".to_owned()
            } else {
                format!("{attachment_count} attachments")
            };
            meta = meta.child(rune_kit::Badge::new(label).build(&t.k));
        }
        bubble = bubble.child(meta);
    }

    if !display.is_empty() {
        bubble = bubble.child(
            text(display)
                .font(t.ui)
                .color(if editing { t.text } else { t.muted })
                .wrap()
                .max_lines(4),
        );
    }

    let actions = queued_actions(run, editing, connected, t);

    let column = col()
        .min_w(0.0)
        .shrink()
        .items_end()
        .child(bubble)
        .child(actions);

    motion::reflow(
        row()
            .key(format!("queued-row-{}", run.0))
            .w_full()
            .justify_end()
            .hover_group()
            .transition(motion::tucked_deck())
            .child(el().w_frac(1.0 - BUBBLE_FRAC).a11y_hidden())
            .child(column),
    )
}

fn queued_actions(
    run: viz_client::protocol::RunId,
    editing: bool,
    connected: bool,
    t: &Theme,
) -> El {
    let send_now = rune_kit::Button::new("Send now")
        .variant(rune_kit::ButtonVariant::GhostMuted)
        .control_size(crate::theme::Size::Micro.kit())
        .icon(&ui::ARROW_UP)
        .disabled(!connected)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::SteerQueued(run)), tx);
        })
        .build(&t.k)
        .key("send-now");

    let edit = rune_kit::IconButton::new(&ui::PENCIL)
        .help("Edit queued message")
        .variant(IconButtonVariant::GhostMuted)
        .control_size(Size::Micro.kit())
        .expanded(editing)
        .disabled(!connected)
        .on_press_tx(move |s: &mut Desktop, tx| {
            if editing {
                s.dispatch(Msg::Composer(ComposerMsg::CancelEditQueued), tx);
            } else {
                s.dispatch(Msg::Composer(ComposerMsg::EditQueued(run)), tx);
            }
        })
        .build(&t.k)
        .key("edit");

    let remove = rune_kit::IconButton::new(&ui::CLOSE)
        .help("Remove queued message")
        .variant(IconButtonVariant::GhostMuted)
        .control_size(Size::Micro.kit())
        .disabled(!connected)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::RemoveQueued(run)), tx);
        })
        .build(&t.k)
        .key("remove");

    row()
        .key(format!("queued-actions-{}", run.0))
        .mt(S2)
        .gap(S4)
        .center_y()
        .justify_end()
        .child(send_now)
        .child(edit)
        .child(remove)
}
