//! Composer card surface: `TextEdit` with atomic `Mark::Pill` inline chips,
//! drag-and-drop accent frame, prompt history recall, length warning, and
//! footer controls (no voice input).

use rune::{Color, El, Key, KeyEvent, Tx, col, icon, row, text, text_input};

use super::{
    actions::{PillPalette, primary_actions},
    attachments, completion, markdown, toolbar,
};
use crate::{
    app::Desktop,
    attachments as attachment_io,
    model::{PROMPT_CHAR_LIMIT, PROMPT_WARN_LIMIT, available_commands},
    motion,
    theme::{
        HAIRLINE, ICON_SMALL, RADIUS_SM, RADIUS_SURFACE, S4, S8, S10, S14, S16, S40, S44, Theme,
    },
    ui,
    update::{ComposerMsg, Msg},
};

/// Maximum visible lines before vertical scrolling inside the composer input.
const MAX_LINES: usize = 9;

/// Builds the flat composer card with inline chips, attachments strip,
/// drag-over highlight, footer controls, and circular primary CTA.
pub fn card(s: &Desktop, is_home: bool, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let placeholder = if !m.connected() {
        "Offline"
    } else if m.composer.editing_queued.is_some() {
        "Update queued message…"
    } else {
        "Ask anything. @ files, $ skills, / commands"
    };

    let field = draft_field(s, placeholder, t);

    let has_attachments = !m.composer.draft.attachments.is_empty();
    let border = if m.composer.drag_over {
        t.accent
    } else if s.widgets.draft.focused {
        t.border_strong
    } else {
        t.border_control
    };

    let attach = (m.connected()
        && (is_home || m.selection.new_thread.is_some() || m.thread_state().is_some()))
    .then(|| attachment_io::button(s, t));
    let access_chip = (!is_home)
        .then(|| crate::model_picker::access::thread_trigger(s, t, layer))
        .flatten();
    let compact_toggle = (!is_home).then(|| compact_chip(s, t)).flatten();

    let footer = row()
        .key("composer-toolbar")
        .w_full()
        .h(S40)
        .px(S8)
        .pb(S8)
        .gap(S4)
        .center_y()
        .instant_changes()
        .children(attach)
        .child(rune::spacer())
        .child(
            row()
                .key("composer-right-controls")
                .gap(S8)
                .center_y()
                .min_w(0.0)
                .shrink()
                .child(toolbar::toolbar(s, t, layer).shrink().min_w(0.0))
                .children(access_chip)
                .children(compact_toggle)
                .child(primary_actions(s, t)),
        );

    let card = col()
        .key("composer-card")
        .paint_order(30)
        .w_full()
        .radius(RADIUS_SURFACE)
        .fill(t.surface)
        .border(HAIRLINE, border)
        .children(attachments::strip(s, t))
        .children(editing_queued_banner(s, t))
        .child(
            col()
                .w_full()
                .px(S16)
                .pt(if has_attachments { S8 } else { S14 })
                .pb(S8)
                .min_h(S44)
                .child(rune::auto_grow(field, 1, MAX_LINES)),
        )
        .children(length_warning(s, t))
        .child(footer);

    let card = motion::shared_surface("composer-shell", card);

    if m.can_attach() || (is_home && m.connected()) {
        card.on_drop_files(attachment_io::drop_files)
            .on_drop_hover(|s: &mut Desktop, over, tx| {
                s.dispatch(Msg::Composer(ComposerMsg::SetDragOver(over)), tx);
            })
    } else {
        card
    }
}

fn draft_field(s: &Desktop, placeholder: &'static str, t: &Theme) -> El {
    let pill_theme = PillPalette::from_theme(t);
    let md = markdown::decorate(&s.widgets.draft, t);
    text_input(&s.widgets.draft, t.body)
        .key("draft")
        .label("Message")
        .w_full()
        .color(t.text)
        .caret(t.accent, t.k.selection())
        .placeholder(placeholder, t.faint)
        .decorations(md.spans)
        .strikes(md.strikes)
        .pill_style(move |id| pill_theme.style(id))
        .spellcheck(true)
        .on_edit(|s: &mut Desktop| &mut s.widgets.draft)
        .on_input_change(|s: &mut Desktop, tx| {
            s.sync_draft_text();
            let text = s.widgets.draft.text.clone();
            s.dispatch(Msg::Composer(ComposerMsg::DraftChanged(text)), tx);
            let cursor = s.draft_cursor();
            s.dispatch(
                Msg::Composer(ComposerMsg::PollCompletion {
                    clock: tx.clock(),
                    cursor,
                }),
                tx,
            );
        })
        .on_input_paste(attachment_io::paste)
        .on_input_key(on_draft_key)
        .on_submit(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx);
        })
}

fn length_warning(s: &Desktop, t: &Theme) -> Option<El> {
    let char_count = s.model.composer.draft.text.chars().count();
    (char_count > PROMPT_WARN_LIMIT).then(|| {
        let over = char_count > PROMPT_CHAR_LIMIT;
        let color = if over { t.danger } else { t.warning };
        let label = if over {
            format!("Prompt exceeds limit ({char_count} / {PROMPT_CHAR_LIMIT} chars)")
        } else {
            format!("Long prompt ({char_count} / {PROMPT_CHAR_LIMIT} chars)")
        };
        row()
            .key("length-warning")
            .w_full()
            .px(S16)
            .pb(S4)
            .child(text(label).font(t.small).color(color).role("status"))
    })
}

fn editing_queued_banner(s: &Desktop, t: &Theme) -> Option<El> {
    s.model.composer.editing_queued.is_some().then(|| {
        row()
            .key("editing-queued-banner")
            .w_full()
            .px(S16)
            .pt(S10)
            .gap(S8)
            .center_y()
            .child(
                icon(&ui::PENCIL, ICON_SMALL)
                    .color(t.accent_text)
                    .a11y_hidden(),
            )
            .child(
                text("Editing queued message")
                    .font(t.small_medium)
                    .color(t.accent_text)
                    .grow(1.0),
            )
            .child(
                rune_kit::Button::new("Cancel")
                    .variant(rune_kit::ButtonVariant::GhostMuted)
                    .control_size(crate::theme::Size::Micro.kit())
                    .on_press_tx(|s: &mut Desktop, tx| {
                        s.dispatch(Msg::Composer(ComposerMsg::CancelEditQueued), tx);
                    })
                    .build(&t.k)
                    .key("cancel-edit-queued"),
            )
    })
}

fn on_draft_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if crate::requests::on_draft_key(s, ev, tx) || completion::on_key(s, ev, tx) {
        return true;
    }
    if markdown::handle_key(&mut s.widgets.draft, ev) {
        s.sync_draft_text();
        let text = s.widgets.draft.text.clone();
        s.dispatch(Msg::Composer(ComposerMsg::DraftChanged(text)), tx);
        return true;
    }
    if ev.key == Key::Escape {
        if s.model.composer.editing_queued.is_some() {
            s.dispatch(Msg::Composer(ComposerMsg::CancelEditQueued), tx);
            return true;
        }
        if s.model.busy()
            && s.widgets.draft.text.is_empty()
            && s.model.composer.open_picker.is_none()
        {
            s.dispatch(Msg::Composer(ComposerMsg::AbortAt(tx.clock())), tx);
            return true;
        }
    }
    if ev.key == Key::Enter && (ev.mods.ctrl || ev.mods.cmd) && !ev.mods.shift && s.model.busy() {
        s.dispatch(Msg::Composer(ComposerMsg::QueueInput), tx);
        return true;
    }
    if !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt && !ev.mods.shift {
        let empty = s.widgets.draft.text.is_empty();
        let browsing_single_line = s.model.composer.history_cursor.is_some()
            && !s.widgets.draft.text.contains('\n')
            && s.widgets.draft.anchor == s.widgets.draft.focus
            && (s.widgets.draft.focus == 0 || s.widgets.draft.focus == s.widgets.draft.text.len());
        match ev.key {
            Key::Up if empty || browsing_single_line => {
                s.dispatch(Msg::Composer(ComposerMsg::HistoryStep { up: true }), tx);
                return true;
            }
            Key::Down if browsing_single_line => {
                s.dispatch(Msg::Composer(ComposerMsg::HistoryStep { up: false }), tx);
                return true;
            }
            _ => {}
        }
    }
    false
}

fn compact_chip(s: &Desktop, t: &Theme) -> Option<El> {
    let usage = s.model.thread_state()?.usage.as_ref()?;
    if usage.size == 0 || u128::from(usage.used) * 100 < u128::from(usage.size) * 75 {
        return None;
    }
    let supports_compact = available_commands(&s.model)
        .iter()
        .any(|cmd| cmd.name.eq_ignore_ascii_case("compact"));
    if !supports_compact {
        return None;
    }
    let active = s.model.composer.compact_before_send;
    let label = if active { "Compact" } else { "Keep" };
    Some(
        row()
            .key("compact-toggle")
            .role("button")
            .label("Compact before send")
            .a11y_value(if active { "On" } else { "Off" })
            .h(26.0)
            .px(S8)
            .gap(S4)
            .center_y()
            .radius(RADIUS_SM)
            .fill(if active {
                t.warning.alpha(0.14)
            } else {
                Color::TRANSPARENT
            })
            .border(
                HAIRLINE,
                if active {
                    t.warning.alpha(0.32)
                } else {
                    t.hairline
                },
            )
            .hover_group()
            .hover(t.hover_wash)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Composer(ComposerMsg::ToggleCompactBeforeSend), tx);
            })
            .child(
                icon(&ui::MINIMIZE2, ICON_SMALL)
                    .color(if active { t.warning } else { t.muted })
                    .a11y_hidden(),
            )
            .child(
                text(label)
                    .font(t.small_medium)
                    .color(if active { t.warning } else { t.muted })
                    .a11y_hidden(),
            ),
    )
}
