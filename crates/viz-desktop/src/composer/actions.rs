//! Composer primary circular CTA buttons (Send, Stop, Steer, Queue, Update,
//! Opening/Waiting spinner) and semantic `Mark::Pill` color palette.

use rune::{Color, El, PillStyle, el, icon, row};
use rune_kit::IconButtonVariant;

use crate::{
    app::Desktop,
    motion,
    theme::{HAIRLINE, ICON_MD, RADIUS_PILL, S2, S6, S32, Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Diameter of the primary circular CTA button (`32×32pt`).
const PRIMARY_CIRCLE: f32 = S32;
/// Side length of the rounded square stop glyph (`11×11pt`).
const STOP_GLYPH: f32 = 11.0;

/// Renders the composer footer's right-most primary action cluster.
pub(super) fn primary_actions(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let actions = row()
        .key("composer-actions")
        .gap(S6)
        .center_y()
        .instant_changes();

    let sending_draft = m.selection.new_thread.as_ref().is_some_and(|d| d.sending);
    let reading_attachments = m
        .attachment_target()
        .is_some_and(|target| m.composer.attachment_reads.contains(&target))
        || m.composer.draft.has_pending_or_failed_attachments();
    if sending_draft || reading_attachments {
        let tip = if sending_draft {
            "Opening session…"
        } else {
            "Waiting for attachments…"
        };
        let held = row()
            .key("send")
            .role("button")
            .label(tip)
            .help(tip)
            .size(PRIMARY_CIRCLE, PRIMARY_CIRCLE)
            .center()
            .radius(RADIUS_PILL)
            .fill(t.text.alpha(0.12))
            .disabled(true)
            .child(ui::radar(ICON_MD, t.muted, t));
        return actions.child(held);
    }

    if m.composer.editing_queued.is_some() {
        let enabled = m.can_send();
        let (fill, ink) = if enabled {
            (t.accent, t.on_accent)
        } else {
            (t.text.alpha(0.12), t.faint)
        };
        let mut update = circle_action(
            "send",
            "Update queued message",
            "Update queued message (Enter)",
            &ui::CHECK,
            fill,
            ink,
            enabled,
            t,
        );
        if enabled {
            update = update.on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx);
            });
        }
        return actions.child(update);
    }

    if m.busy() {
        return busy_actions(actions, s, t);
    }

    let enabled = m.can_send();
    let (fill, ink) = if enabled {
        (t.accent, t.on_accent)
    } else {
        (t.text.alpha(0.12), t.faint)
    };
    let mut send = circle_action(
        "send",
        "Send",
        "Send (Enter)",
        &ui::ARROW_UP,
        fill,
        ink,
        enabled,
        t,
    );
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
            .label("Stop generating")
            .help("Stop generating (Esc)")
            .shortcut_hint("Esc")
            .size(PRIMARY_CIRCLE, PRIMARY_CIRCLE)
            .center()
            .radius(RADIUS_PILL)
            .fill(t.danger.alpha(0.92))
            .hover(t.danger)
            .disabled(!m.connected())
            .focusable(m.connected())
            .focus_ring(HAIRLINE, S2, t.accent)
            .press_scale(motion::PRESS_SCALE)
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Composer(ComposerMsg::AbortAt(tx.clock())), tx);
            })
            .child(
                el().size(STOP_GLYPH, STOP_GLYPH)
                    .radius(2.0)
                    .fill(t.on_danger)
                    .a11y_hidden(),
            );
        return actions.child(stop);
    }

    let queue_hint = format!("Queue message ({}+Enter)", ui::COMMAND_KEY);
    let queue_btn = ui::icon_button(
        &ui::LIST_PLUS,
        &queue_hint,
        IconButtonVariant::GhostMuted,
        Size::Compact,
    )
    .disabled(!m.connected())
    .on_press_tx(|s: &mut Desktop, tx| {
        s.dispatch(Msg::Composer(ComposerMsg::QueueInput), tx);
    })
    .build(&t.k)
    .key("queue");

    let stop_btn = row()
        .key("stop")
        .role("button")
        .label("Stop generating")
        .help(format!("Stop generating  {}", ui::shortcut(".")))
        .size(Size::Compact.height(), Size::Compact.height())
        .center()
        .radius(RADIUS_PILL)
        .fill(t.text.alpha(0.10))
        .hover(t.hover_wash)
        .disabled(!m.connected())
        .focusable(m.connected())
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .press_scale(motion::PRESS_SCALE)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::AbortAt(tx.clock())), tx);
        })
        .child(el().size(9.0, 9.0).radius(2.0).fill(t.danger).a11y_hidden());

    let steer_tip = format!("Steer active turn · {}+Enter to queue", ui::COMMAND_KEY);
    let steer = circle_action(
        "send",
        "Steer active turn",
        &steer_tip,
        &ui::CORNER_UP_RIGHT,
        t.steer,
        t.on_accent,
        true,
        t,
    )
    .on_press_tx(|s: &mut Desktop, tx| {
        s.dispatch(Msg::Composer(ComposerMsg::SendInput), tx);
    });

    actions.child(stop_btn).child(queue_btn).child(steer)
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared circular CTA builder accepts explicit tooltip, icon, and palette"
)]
fn circle_action(
    key: &'static str,
    label: &'static str,
    tooltip: &str,
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
        .help(tooltip)
        .size(PRIMARY_CIRCLE, PRIMARY_CIRCLE)
        .center()
        .radius(RADIUS_PILL)
        .fill(fill)
        .disabled(!enabled)
        .focusable(enabled)
        .focus_ring(HAIRLINE, S2, t.accent)
        .child(icon(glyph, ICON_MD + 1.0).color(ink).a11y_hidden());
    if enabled {
        button = button.hover_group().press_scale(motion::PRESS_SCALE);
    }
    button
}

/// Captures theme colors so the `pill_style` closure stays `'static` and
/// zero-alloc.
#[derive(Clone, Copy)]
pub(super) struct PillPalette {
    info: Color,
    accent: Color,
    accent_text: Color,
    delegated: Color,
    steer: Color,
    success: Color,
    muted: Color,
    danger: Color,
    chip_fill: Color,
    text_secondary: Color,
}

impl PillPalette {
    /// Extracts base16 semantic pill tokens from the active [`Theme`].
    pub(super) const fn from_theme(t: &Theme) -> Self {
        Self {
            info: t.info,
            accent: t.accent,
            accent_text: t.accent_text,
            delegated: t.delegated,
            steer: t.steer,
            success: t.success,
            muted: t.muted,
            danger: t.danger,
            chip_fill: t.chip_fill,
            text_secondary: t.text_secondary,
        }
    }

    /// Maps a typed pill id (`file:`, `dir:`, `thread:`, `env:`, `model:`,
    /// `skill:`, `pr:`, `issue:`) to its inline `Mark::Pill` fill, text ink,
    /// and leading icon.
    pub(super) fn style(&self, id: &str) -> PillStyle {
        if let Some(path) = id.strip_prefix("file:") {
            return PillStyle::default()
                .bg(self.info.alpha(0.12))
                .fg(self.info)
                .icon(rune_kit::file_icon(path));
        }
        if id.starts_with("dir:") {
            return PillStyle::default()
                .bg(self.info.alpha(0.12))
                .fg(self.info)
                .icon(&ui::FOLDER);
        }
        if id.starts_with("thread:") {
            return PillStyle::default()
                .bg(self.accent.alpha(0.12))
                .fg(self.accent_text)
                .icon(&ui::THREAD);
        }
        if id.starts_with("env:") {
            return PillStyle::default()
                .bg(self.accent.alpha(0.12))
                .fg(self.accent_text)
                .icon(&ui::SERVER);
        }
        if id.starts_with("model:") {
            return PillStyle::default()
                .bg(self.delegated.alpha(0.12))
                .fg(self.delegated)
                .icon(&ui::CPU);
        }
        if id.starts_with("skill:") {
            return PillStyle::default()
                .bg(self.steer.alpha(0.12))
                .fg(self.steer)
                .icon(&ui::SPARKLES);
        }
        if let Some(rest) = id.strip_prefix("pr:").or_else(|| id.strip_prefix("issue:")) {
            let is_issue = id.starts_with("issue:");
            let state = rest.split(':').next().unwrap_or("open");
            let (fg, glyph) = match (is_issue, state) {
                (false, "merged") => (self.delegated, &ui::GIT_MERGE),
                (false, "draft") => (self.muted, &ui::PR_DRAFT),
                (false, "closed") => (self.danger, &ui::PR_CLOSED),
                (false, _) => (self.success, &ui::PR_OPEN),
                (true, "closed") => (self.delegated, &ui::CIRCLE_CHECK),
                (true, _) => (self.success, &ui::CIRCLE_DOT),
            };
            return PillStyle::default().bg(fg.alpha(0.12)).fg(fg).icon(glyph);
        }
        PillStyle::default()
            .bg(self.chip_fill)
            .fg(self.text_secondary)
    }
}
