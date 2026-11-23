//! Access-mode trigger and popover, driven only by the modes the harness offers
//! through ACP (`decisions.md` #6): Full access is the default and missing
//! modes are never faked.
//!
//! The popover is a segmented row with one icon per offered mode and a live
//! description that follows hover, falling back to the chosen mode. The model
//! picker footer uses [`footer_trigger`]; the Home context strip embeds
//! [`access_trigger`], which mounts its own popover.

use rune::{Color, El, Icon, col, icon, row, text};
use rune_kit::{Placement, scrim};
use viz_client::protocol::{AccessKind, AccessMode};

use super::send;
use crate::{
    app::Desktop,
    model::{ACCESS_PICKER, access_summary, access_view},
    theme::{
        HAIRLINE, ICON, ICON_SMALL, RADIUS_CONTROL, RADIUS_SM, S2, S4, S6, S8, S10, S12, S24, S32,
        Theme,
    },
    ui,
    update::{ModelPickerMsg, Msg},
};

/// Element key of the picker footer's access trigger.
pub const FOOTER_TRIGGER: &str = "model-access-trigger";
/// Element key of the Home strip's access trigger.
#[expect(dead_code, reason = "embedded by the Home context strip (Track H)")]
pub const STRIP_TRIGGER: &str = "access-trigger";
/// Popover width.
const WIDTH: f32 = 288.0;

/// The glyph for an access kind.
#[must_use]
pub const fn access_icon(kind: AccessKind) -> &'static Icon {
    match kind {
        AccessKind::Full => &ui::UNLOCK,
        AccessKind::Auto => &ui::SPARKLES,
        AccessKind::AcceptEdits => &ui::SHIELD_CHECK,
        AccessKind::Ask => &ui::SHIELD,
        AccessKind::ReadOnly => &ui::EYE,
    }
}

/// viz's name for an access kind, shown beside a harness's own label.
const fn kind_name(kind: AccessKind) -> &'static str {
    match kind {
        AccessKind::Full => "Full access",
        AccessKind::Auto => "Automatic",
        AccessKind::AcceptEdits => "Accept edits",
        AccessKind::Ask => "Ask first",
        AccessKind::ReadOnly => "Read only",
    }
}

fn trigger(key: &str, mode: &AccessMode, open: bool, height: f32, pad: f32, t: &Theme) -> El {
    let ink = if open { t.text } else { t.muted };
    row()
        .key(key)
        .role("button")
        .label("Access mode")
        .a11y_value(&mode.label)
        .expanded(open)
        .help(format!("Access: {}", mode.label))
        .focusable(false)
        .preserve_focus_on_press(true)
        .h(height)
        .px(pad)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if open {
            t.hover_wash
        } else {
            Color::TRANSPARENT
        })
        .hover(t.hover_wash)
        .hover_group()
        .on_press_tx(send(ModelPickerMsg::ToggleAccess))
        .child(
            icon(access_icon(mode.kind), ICON_SMALL)
                .color(ink)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(&mode.label)
                .font(t.small_medium)
                .color(ink)
                .group_color(t.text)
                .truncate()
                .a11y_hidden(),
        )
        .child(
            crate::motion::chevron(&ui::CHEVRON_UP, ICON_SMALL, open, std::f32::consts::PI)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

/// The model picker footer's access trigger, when the harness offers modes.
pub(super) fn footer_trigger(s: &Desktop, t: &Theme) -> Option<El> {
    let view = access_view(&s.model)?;
    let open = s.model.composer.model_menu.access_open;
    Some(trigger(FOOTER_TRIGGER, view.current, open, 28.0, S8, t))
}

/// A standalone access-mode pill (Home context strip): `24pt` ghost trigger
/// that mounts its popover into `layer` while open.
#[expect(dead_code, reason = "embedded by the Home context strip (Track H)")]
pub fn access_trigger(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let view = access_view(&s.model)?;
    let open = s.model.composer.open_picker.as_deref() == Some(ACCESS_PICKER);
    if open {
        layer.extend(popover(s, STRIP_TRIGGER, Placement::TopEnd, t));
    }
    Some(trigger(STRIP_TRIGGER, view.current, open, S24, S6, t))
}

/// The access popover anchored to `anchor`: a scrim and the flat card.
pub fn popover(s: &Desktop, anchor: &str, placement: Placement, t: &Theme) -> Vec<El> {
    let Some(view) = access_view(&s.model) else {
        return Vec::new();
    };
    let current = view.current_index();
    let preview = s
        .model
        .composer
        .model_menu
        .access_preview
        .filter(|index| *index < view.modes.len())
        .or(current)
        .and_then(|index| view.modes.get(index))
        .unwrap_or(view.current);
    let mut track = row()
        .key("access-modes")
        .role("radiogroup")
        .label("Access mode")
        .w_full()
        .padding(S2)
        .gap(S2)
        .radius(RADIUS_CONTROL)
        .fill(t.surface_recessed);
    for (index, mode) in view.modes.iter().enumerate() {
        let on = current == Some(index);
        track = track.child(
            row()
                .key(format!("access-mode-{index}"))
                .role("radio")
                .label(&mode.label)
                .help(&mode.label)
                .checked(on)
                .focusable(false)
                .preserve_focus_on_press(true)
                .grow(1.0)
                .h(S32)
                .center()
                .radius(RADIUS_SM)
                .fill(if on {
                    t.surface_raised
                } else {
                    Color::TRANSPARENT
                })
                .border(HAIRLINE, if on { t.hairline } else { Color::TRANSPARENT })
                .hover(t.hover_wash)
                .on_hover_change(move |s: &mut Desktop, inside, tx| {
                    let preview = inside.then_some(index);
                    s.dispatch(Msg::ModelPicker(ModelPickerMsg::PreviewAccess(preview)), tx);
                })
                .on_press_tx(send(ModelPickerMsg::ChooseAccess(index)))
                .child(
                    icon(access_icon(mode.kind), ICON)
                        .color(if on { t.text_strong } else { t.muted })
                        .a11y_hidden(),
                ),
        );
    }
    let kind = kind_name(preview.kind);
    let mut title = row()
        .gap(S6)
        .center_y()
        .child(text(&preview.label).font(t.ui_medium).color(t.text_strong));
    if !preview.label.eq_ignore_ascii_case(kind) {
        title = title.child(text(kind).font(t.micro).color(t.faint));
    }
    let body = col().w_full().padding(S12).gap(S10).child(track).child(
        col()
            .key("access-preview")
            .w_full()
            .gap(S4)
            .instant_changes()
            .child(title)
            .child(
                text(access_summary(preview.kind))
                    .font(t.small)
                    .color(t.muted)
                    .wrap(),
            ),
    );
    let close = |s: &mut Desktop| s.dispatch_local(Msg::ModelPicker(ModelPickerMsg::CloseAccess));
    vec![
        scrim("access-scrim", close),
        ui::anchored_popover("access-popover", anchor, placement, WIDTH, body, t).paint_order(42),
    ]
}
