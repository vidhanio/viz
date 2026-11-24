//! Interactive `components` and `components-reduced` gallery scenes exercising
//! every design token in [`crate::theme`] and every component in [`crate::ui`].

mod controls;
mod surfaces;
mod tokens;

use rune::{El, TextEdit, col, row, text};
use rune_kit::ButtonVariant;

use self::{
    controls::{badges_card, buttons_card, controls_card},
    surfaces::{feedback_card, rows_card, surfaces_card},
    tokens::{identity_card, palette_card, typography_card},
};
use crate::{
    app::Desktop,
    motion,
    motion_scene::MotionDemo,
    theme::{CARD_INSET, DIALOG_MAX_WIDTH, HAIRLINE, RADIUS_CARD, S4, S8, S12, S16, Size, Theme},
    ui::{self, ScrollMode, Tone},
};

#[derive(Debug, Default)]
pub(super) struct GalleryOverlays {
    pub(super) popover: bool,
    pub(super) dialog: bool,
    pub(super) toast: bool,
}

#[derive(Debug)]
pub(super) struct GallerySwitches {
    pub(super) shimmer: bool,
    pub(super) auto_scroll: bool,
    pub(super) plan_mode: bool,
}

impl Default for GallerySwitches {
    fn default() -> Self {
        Self {
            shimmer: true,
            auto_scroll: true,
            plan_mode: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct GallerySections {
    pub(super) pinned_open: bool,
    pub(super) snoozed_open: bool,
    pub(super) banner_visible: bool,
}

impl Default for GallerySections {
    fn default() -> Self {
        Self {
            pinned_open: true,
            snoozed_open: false,
            banner_visible: true,
        }
    }
}

/// Interactive state for the `components` and `components-reduced` gallery
/// scenes.
#[derive(Debug)]
pub struct GalleryState {
    pub(super) overlays: GalleryOverlays,
    pub(super) switches: GallerySwitches,
    pub(super) sections: GallerySections,
    pub(super) segmented_mode: usize,
    pub(super) diff_step: usize,
    pub(super) active_btn: bool,
    pub(super) selected_row: usize,
    pub(super) search_filled: TextEdit,
    pub(super) search_empty: TextEdit,
    pub(super) combo_search: TextEdit,
    pub(super) field_ok: TextEdit,
    pub(super) field_err: TextEdit,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            overlays: GalleryOverlays::default(),
            switches: GallerySwitches::default(),
            sections: GallerySections::default(),
            segmented_mode: 1,
            diff_step: 0,
            active_btn: true,
            selected_row: 1,
            search_filled: TextEdit::new("filter"),
            search_empty: TextEdit::default(),
            combo_search: TextEdit::default(),
            field_ok: TextEdit::new("viz-desktop"),
            field_err: TextEdit::new("/missing/repo"),
        }
    }
}

pub(super) fn with_gallery(s: &mut Desktop, f: impl FnOnce(&mut GalleryState)) {
    if let Some(MotionDemo::Components(state)) = s.motion_demo.as_mut() {
        f(state);
    }
}

pub(super) fn edit_gallery(
    s: &mut Desktop,
    pick: impl FnOnce(&mut GalleryState) -> &mut TextEdit,
) -> &mut TextEdit {
    let Some(MotionDemo::Components(state)) = s.motion_demo.as_mut() else {
        unreachable!("gallery editor invoked outside components scene");
    };
    pick(state)
}

/// Renders the complete UI component gallery inside viz's flat base16 theme.
pub fn view(state: &GalleryState, reduced: bool, viewport: [f32; 2], t: &Theme) -> El {
    let mut overlays = Vec::new();
    let col_tokens = col()
        .key("gallery-col-tokens")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S12)
        .child(palette_card(t))
        .child(typography_card(t))
        .child(identity_card(t));

    let col_controls = col()
        .key("gallery-col-controls")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S12)
        .child(buttons_card(state, t))
        .child(badges_card(state, t))
        .child(controls_card(state, t));

    let col_surfaces = col()
        .key("gallery-col-surfaces")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S12)
        .child(rows_card(state, t))
        .child(surfaces_card(state, t))
        .child(feedback_card(state, reduced, t));

    let grid = row()
        .key("gallery-grid")
        .w_full()
        .padding(S12)
        .gap(S12)
        .items_start()
        .child(col_tokens)
        .child(col_controls)
        .child(col_surfaces);

    let scroll = ui::scroll_viewport("gallery-scroll", ScrollMode::Top, grid, t);

    let page = col()
        .key("gallery-page")
        .w_full()
        .h_full()
        .fill(t.canvas)
        .child(gallery_toolbar(state, reduced, t))
        .child(ui::rule(t).key("gallery-rule"))
        .child(scroll);

    if state.overlays.popover {
        overlays.push(gallery_popover(viewport, t));
    }
    if state.overlays.toast {
        overlays.push(gallery_toast(t));
    }
    if state.overlays.dialog {
        gallery_dialog(t, &mut overlays);
    }

    rune_kit::root(page, overlays).hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
}

fn gallery_toolbar(state: &GalleryState, reduced: bool, t: &Theme) -> El {
    let mode_badge = if reduced {
        rune_kit::Badge::new("Reduced motion").build(&t.k)
    } else {
        rune_kit::Badge::new("Flat base16 · 1px hairlines").build(&t.k)
    };
    row()
        .key("gallery-toolbar")
        .w_full()
        .h(44.0)
        .px(S16)
        .gap(S8)
        .center_y()
        .fill(t.sidebar_bg)
        .child(
            text("UI component gallery")
                .font(t.title)
                .color(t.text_strong),
        )
        .child(mode_badge)
        .child(rune::spacer())
        .child(
            rune_kit::Button::new(if state.switches.shimmer {
                "Shimmer: on"
            } else {
                "Shimmer: off"
            })
            .variant(ButtonVariant::Outline)
            .control_size(Size::Small.kit())
            .state(if state.switches.shimmer {
                rune_kit::State::Pressed
            } else {
                rune_kit::State::Rest
            })
            .on_press(|s: &mut Desktop| {
                with_gallery(s, |g| g.switches.shimmer = !g.switches.shimmer);
            })
            .build(&t.k)
            .key("btn-gallery-shimmer"),
        )
        .child(
            rune_kit::Button::new("Popover")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Small.kit())
                .state(if state.overlays.popover {
                    rune_kit::State::Pressed
                } else {
                    rune_kit::State::Rest
                })
                .on_press(|s: &mut Desktop| {
                    with_gallery(s, |g| g.overlays.popover = !g.overlays.popover);
                })
                .build(&t.k)
                .key("btn-gallery-popover"),
        )
        .child(
            rune_kit::Button::new("Dialog")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Small.kit())
                .state(if state.overlays.dialog {
                    rune_kit::State::Pressed
                } else {
                    rune_kit::State::Rest
                })
                .on_press(|s: &mut Desktop| {
                    with_gallery(s, |g| g.overlays.dialog = !g.overlays.dialog);
                })
                .build(&t.k)
                .key("btn-gallery-dialog"),
        )
        .child(
            rune_kit::Button::new("Toast")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Small.kit())
                .state(if state.overlays.toast {
                    rune_kit::State::Pressed
                } else {
                    rune_kit::State::Rest
                })
                .on_press(|s: &mut Desktop| {
                    with_gallery(s, |g| g.overlays.toast = !g.overlays.toast);
                })
                .build(&t.k)
                .key("btn-gallery-toast"),
        )
}

pub(super) fn gallery_card(key: &str, title: &str, subtitle: &str, body: El, t: &Theme) -> El {
    col()
        .key(key)
        .w_full()
        .padding(CARD_INSET)
        .gap(S8)
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .child(
            row()
                .w_full()
                .gap(S8)
                .center_y()
                .child(text(title).font(t.section_heading).color(t.text_strong))
                .child(rune::spacer())
                .child(text(subtitle).font(t.micro).color(t.muted).truncate()),
        )
        .child(ui::rule(t))
        .child(body)
}

fn gallery_popover(viewport: [f32; 2], t: &Theme) -> El {
    let body = col()
        .w_full()
        .padding(S12)
        .gap(S8)
        .child(
            row()
                .w_full()
                .center_y()
                .child(text("Anchored popover").font(t.title).color(t.text_strong))
                .child(rune::spacer())
                .child(rune_kit::Keybind::new(&[ui::COMMAND_KEY, "K"]).build(&t.k)),
        )
        .child(ui::rule(t))
        .child(
            text("Flat opaque surface_raised framed by a 1px border_overlay hairline.")
                .font(t.small)
                .color(t.muted)
                .wrap(),
        )
        .child(
            row().w_full().justify_end().child(
                rune_kit::Button::new("Dismiss")
                    .variant(ButtonVariant::Outline)
                    .control_size(Size::Small.kit())
                    .on_press(|s: &mut Desktop| with_gallery(s, |g| g.overlays.popover = false))
                    .build(&t.overlay)
                    .key("btn-gallery-popover-close"),
            ),
        );
    ui::anchored_popover(
        ui::popover_frame("gallery-popover", 264.0, t),
        "btn-gallery-popover",
        (rune::Side::Bottom, rune::Align::End),
        S4,
        viewport,
        body,
    )
}

fn gallery_dialog(t: &Theme, overlays: &mut Vec<El>) {
    let actions = ui::dialog_actions([
        rune_kit::Button::new("Cancel")
            .variant(ButtonVariant::Ghost)
            .control_size(Size::Default.kit())
            .on_press(|s: &mut Desktop| with_gallery(s, |g| g.overlays.dialog = false))
            .build(&t.overlay),
        rune_kit::Button::new("Confirm")
            .variant(ButtonVariant::Submit)
            .control_size(Size::Default.kit())
            .on_press(|s: &mut Desktop| with_gallery(s, |g| g.overlays.dialog = false))
            .build(&t.overlay),
    ]);
    let body = ui::dialog_body(
        [
            text("Dialogs use surface_raised, 1px border_overlay, and a translucent scrim.")
                .font(t.body)
                .color(t.text_secondary)
                .wrap(),
        ],
        actions,
    );
    rune_kit::Dialog::new("gallery-modal", "Flat modal dialog")
        .open(true)
        .fit()
        .width(DIALOG_MAX_WIDTH)
        .on_close(|s: &mut Desktop| with_gallery(s, |g| g.overlays.dialog = false))
        .description("Zero drop shadow, crisp hairline frame, Escape to dismiss.")
        .child(ui::dialog_subject(
            "/home/user/projects/viz/crates/viz-desktop",
            t,
        ))
        .child(body)
        .build(&t.overlay, overlays);
}

fn gallery_toast(t: &Theme) -> El {
    rune_kit::Toast::new("gallery-toast", "Thread settled")
        .lead(Tone::Success.mark(t))
        .description("Moved \"Audit theme tokens\" to Settled.")
        .action(&t.overlay, "Undo", true, |s: &mut Desktop, _tx| {
            with_gallery(s, |g| g.overlays.toast = false);
        })
        .on_close(|s: &mut Desktop| with_gallery(s, |g| g.overlays.toast = false))
        .build(&t.overlay)
        .pin(None, None, Some(S16), Some(S16))
}

pub(super) fn swatch_chip(label: &str, fill: rune::Color, border: rune::Color, t: &Theme) -> El {
    col()
        .gap(S4)
        .items_center()
        .child(
            rune::el()
                .size(28.0, 18.0)
                .radius(4.0)
                .fill(fill)
                .border(HAIRLINE, border),
        )
        .child(text(label).font(t.nano).color(t.muted))
}
