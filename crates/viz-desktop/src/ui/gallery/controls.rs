//! Buttons, icon buttons, badges, keycaps, diff stats, inputs, switches,
//! segmented controls, and form fields column for the `components` gallery.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, State};

use super::{GalleryState, edit_gallery, gallery_card, with_gallery};
use crate::{
    app::Desktop,
    theme::{RADIUS_PILL, S6, S8, Size, Theme},
    ui,
};

const DIFF_STEPS: [(u32, u32); 3] = [(42, 13), (128, 29), (7, 2)];

pub(super) fn buttons_card(state: &GalleryState, t: &Theme) -> El {
    let body = col()
        .w_full()
        .gap(S8)
        .child(button_variants(state, t))
        .child(button_sizes_and_disabled(t))
        .child(icon_and_inline_buttons(t))
        .child(
            Button::new("Full-width primary action")
                .variant(ButtonVariant::Submit)
                .control_size(Size::Small.kit())
                .build(&t.k)
                .w_full(),
        );
    gallery_card(
        "card-buttons",
        "Buttons & icon buttons",
        "All sizes + states",
        body,
        t,
    )
}

fn button_variants(state: &GalleryState, t: &Theme) -> El {
    let primary = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            Button::new("Submit")
                .variant(ButtonVariant::Submit)
                .control_size(Size::Compact.kit())
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.active_btn = !g.active_btn))
                .build(&t.k)
                .key("btn-gal-submit"),
        )
        .child(
            Button::new("Neutral")
                .variant(ButtonVariant::Neutral)
                .control_size(Size::Compact.kit())
                .build(&t.k)
                .key("btn-gal-neutral"),
        )
        .child(
            Button::new("Outline")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Compact.kit())
                .state(if state.active_btn {
                    State::Pressed
                } else {
                    State::Rest
                })
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.active_btn = !g.active_btn))
                .build(&t.k)
                .key("btn-gal-outline"),
        )
        .child(
            Button::new("Danger")
                .variant(ButtonVariant::Danger)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        )
        .child(
            Button::new("Warn")
                .variant(ButtonVariant::Warning)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        );

    let saving_spinner = ui::spinner(t);
    let ghost = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            Button::new("Ghost")
                .variant(ButtonVariant::Ghost)
                .control_size(Size::Compact.kit())
                .shortcut(ui::shortcut("N"))
                .build(&t.k),
        )
        .child(
            Button::new("Muted")
                .variant(ButtonVariant::GhostMuted)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        )
        .child(
            Button::new("Faint")
                .variant(ButtonVariant::GhostFaint)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        )
        .child(
            Button::new("New")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Compact.kit())
                .icon(&ui::PLUS)
                .build(&t.k),
        )
        .child(
            Button::new("Saving")
                .variant(ButtonVariant::Loading)
                .control_size(Size::Compact.kit())
                .label_with(move |font, ink| {
                    row()
                        .gap(S6)
                        .center_y()
                        .child(saving_spinner)
                        .child(text("Saving").font(font).color(ink))
                })
                .build(&t.k),
        );

    col().w_full().gap(S6).child(primary).child(ghost)
}

fn button_sizes_and_disabled(t: &Theme) -> El {
    let sizes = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            Button::new("Micro")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Micro.kit())
                .build(&t.k),
        )
        .child(
            Button::new("Small")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Small.kit())
                .build(&t.k),
        )
        .child(
            Button::new("Compact")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        )
        .child(
            Button::new("Large")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Large.kit())
                .build(&t.k),
        );

    let disabled = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            Button::new("Submit")
                .variant(ButtonVariant::Submit)
                .control_size(Size::Small.kit())
                .disabled(true)
                .build(&t.k),
        )
        .child(
            Button::new("Outline")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Small.kit())
                .disabled(true)
                .build(&t.k),
        )
        .child(
            Button::new("Ghost")
                .variant(ButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .disabled(true)
                .build(&t.k),
        )
        .child(text("Disabled").font(t.micro).color(t.faint));

    col().w_full().gap(S6).child(sizes).child(disabled)
}

fn icon_and_inline_buttons(t: &Theme) -> El {
    let icons = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            IconButton::new(&ui::CLOSE)
                .help("Tiny close")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Tiny.kit())
                .build(&t.k),
        )
        .child(
            IconButton::new(&ui::PLUS)
                .help("Micro add")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Micro.kit())
                .build(&t.k),
        )
        .child(
            IconButton::new(&ui::PIN)
                .help("Pin active")
                .variant(IconButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .expanded(true)
                .build(&t.k),
        )
        .child(
            IconButton::new(&ui::LIST_FILTER)
                .help("Filter")
                .variant(IconButtonVariant::Neutral)
                .control_size(Size::Compact.kit())
                .build(&t.k),
        )
        .child(
            IconButton::new(&ui::ARROW_UP)
                .help("Send")
                .variant(IconButtonVariant::Submit)
                .control_size(Size::Default.kit())
                .build(&t.k)
                .radius(RADIUS_PILL),
        )
        .child(
            IconButton::new(&ui::ARROW_UP)
                .help("Send disabled")
                .variant(IconButtonVariant::Submit)
                .control_size(Size::Default.kit())
                .disabled(true)
                .build(&t.k)
                .radius(RADIUS_PILL),
        )
        .child(
            IconButton::new(&ui::SETTINGS)
                .help("Disabled")
                .variant(IconButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .disabled(true)
                .build(&t.k),
        );
    let inline = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            Button::new("Copy")
                .variant(ButtonVariant::GhostMuted)
                .control_size(Size::Micro.kit())
                .icon(&ui::COPY)
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.diff_step += 1))
                .build(&t.k),
        )
        .child(
            Button::new("Retry")
                .variant(ButtonVariant::Danger)
                .control_size(Size::Micro.kit())
                .icon(&ui::ROTATE_CW)
                .build(&t.k),
        )
        .child(
            Button::new("Locked")
                .variant(ButtonVariant::GhostMuted)
                .control_size(Size::Micro.kit())
                .disabled(true)
                .build(&t.k),
        );
    col().w_full().gap(S6).child(icons).child(inline)
}

pub(super) fn badges_card(state: &GalleryState, t: &Theme) -> El {
    let variants = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(rune_kit::Badge::new("Neutral").build(&t.k))
        .child(
            rune_kit::Badge::new("Accent")
                .variant(rune_kit::BadgeVariant::Accent)
                .build(&t.k),
        )
        .child(
            rune_kit::Badge::new("Subtle")
                .variant(rune_kit::BadgeVariant::Subtle)
                .build(&t.k),
        );

    let (add, del) = DIFF_STEPS[state.diff_step % DIFF_STEPS.len()];
    let kbd_diff = row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(rune_kit::Keybind::new(&[ui::COMMAND_KEY, "K"]).build(&t.k))
        .child(rune_kit::Keybind::new(&["Shift", "↵"]).build(&t.k))
        .child(ui::with_tooltip(
            rune_kit::Keybind::new(&["Esc"]).build(&t.k),
            "Dismiss overlay",
            Some("Esc"),
        ))
        .child(rune::spacer())
        .child(ui::diff_stat(18, 4, t))
        .child(text("·").font(t.small).color(t.faint))
        .child(ui::diff_stat_rolling("gal-diff", add, del, t))
        .child(
            rune_kit::Button::new("+")
                .variant(ButtonVariant::Outline)
                .control_size(Size::Micro.kit())
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.diff_step += 1))
                .build(&t.k)
                .key("btn-gallery-diff"),
        );

    let body = col().w_full().gap(S8).child(variants).child(kbd_diff);
    gallery_card(
        "card-badges",
        "Badges, keycaps & diff stats",
        "rune-kit + rolling",
        body,
        t,
    )
}

pub(super) fn controls_card(state: &GalleryState, t: &Theme) -> El {
    let search_row = row()
        .w_full()
        .gap(S6)
        .child(
            {
                let input = rune_kit::TextInput::new(&state.search_filled)
                    .placeholder("Filter threads…")
                    .leading_icon(&ui::SEARCH)
                    .width(None)
                    .label("Active search")
                    .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.search_filled));
                if state.search_filled.text.is_empty() {
                    input
                } else {
                    input.clear_button().on_action(|s: &mut Desktop, _tx| {
                        with_gallery(s, |g| g.search_filled.text.clear());
                    })
                }
            }
            .build(&t.k)
            .key("gallery-search-filled")
            .grow(1.0)
            .shrink()
            .min_w(0.0),
        )
        .child(
            rune_kit::TextInput::new(&state.search_empty)
                .placeholder("Disabled search")
                .leading_icon(&ui::SEARCH)
                .width(None)
                .trailing(rune_kit::Keybind::new(&["/"]).build(&t.k))
                .disabled(true)
                .build(&t.k)
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        );

    let switches = row()
        .w_full()
        .gap(S8)
        .items_start()
        .child(
            rune_kit::Switch::new(state.switches.auto_scroll)
                .label("Auto-scroll")
                .description("Follow live turns")
                .on_toggle(|s: &mut Desktop| {
                    with_gallery(s, |g| g.switches.auto_scroll = !g.switches.auto_scroll);
                })
                .build(&t.k)
                .key("sw-gallery-on")
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        )
        .child(
            rune_kit::Switch::new(state.switches.plan_mode)
                .label("Plan mode")
                .on_toggle(|s: &mut Desktop| {
                    with_gallery(s, |g| g.switches.plan_mode = !g.switches.plan_mode);
                })
                .build(&t.k)
                .key("sw-gallery-off"),
        )
        .child(
            rune_kit::Switch::new(true)
                .label("Locked")
                .disabled(true)
                .build(&t.k),
        );

    let seg_row = row().w_full().gap(S8).center_y().child(
        rune_kit::SegmentedControl::new(
            &["Local", "Worktree", "Remote"],
            Some(state.segmented_mode),
        )
        .label("Checkout mode")
        .fit()
        .segment_state(2, rune_kit::State::Disabled)
        .on_pick(|s: &mut Desktop, idx, _tx| with_gallery(s, |g| g.segmented_mode = idx))
        .build(&t.k)
        .key("seg-gallery-mode"),
    );

    let body = col()
        .w_full()
        .gap(S8)
        .child(search_row)
        .child(switches)
        .child(seg_row)
        .child(fields_row(state, t));
    gallery_card(
        "card-controls",
        "Search, switch, segmented & field",
        "Flat controls",
        body,
        t,
    )
}

fn fields_row(state: &GalleryState, t: &Theme) -> El {
    row()
        .w_full()
        .gap(S8)
        .items_start()
        .child(
            rune_kit::Field::new()
                .label("Project name")
                .control(
                    rune_kit::TextInput::new(&state.field_ok)
                        .label("Project name")
                        .placeholder("e.g. viz")
                        .width(None)
                        .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.field_ok))
                        .build(&t.k)
                        .key("gallery-field-ok"),
                )
                .suffix("Shown in sidebar and headers.")
                .build(&t.k)
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        )
        .child(
            rune_kit::Field::new()
                .label("Repository path")
                .control(
                    rune_kit::TextInput::new(&state.field_err)
                        .label("Repository path")
                        .placeholder("/path/to/repo")
                        .width(None)
                        .invalid(true)
                        .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.field_err))
                        .build(&t.k)
                        .key("gallery-field-err"),
                )
                .suffix("Directory does not exist on this environment.")
                .build(&t.k)
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        )
}
