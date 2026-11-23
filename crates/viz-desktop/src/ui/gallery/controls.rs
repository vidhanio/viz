//! Buttons, icon buttons, badges, keycaps, diff stats, inputs, switches,
//! segmented controls, and form fields column for the `components` gallery.

use rune::{El, col, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant};

use super::{GalleryState, edit_gallery, gallery_card, with_gallery};
use crate::{
    app::Desktop,
    theme::{S6, S8, Size, Theme},
    ui::{self, BadgeSize, InlineStyle, SearchStyle, Tone},
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
            ui::button(
                "Full-width primary action",
                ButtonVariant::Submit,
                Size::Small,
            )
            .full_width()
            .build(&t.k),
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
            ui::button("Submit", ButtonVariant::Submit, Size::Compact)
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.active_btn = !g.active_btn))
                .build(&t.k)
                .key("btn-gal-submit"),
        )
        .child(
            ui::button("Neutral", ButtonVariant::Neutral, Size::Compact)
                .build(&t.k)
                .key("btn-gal-neutral"),
        )
        .child(
            ui::button("Outline", ButtonVariant::Outline, Size::Compact)
                .active(state.active_btn)
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.active_btn = !g.active_btn))
                .build(&t.k)
                .key("btn-gal-outline"),
        )
        .child(ui::button("Danger", ButtonVariant::Danger, Size::Compact).build(&t.k))
        .child(ui::button("Warn", ButtonVariant::Warning, Size::Compact).build(&t.k));

    let ghost = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            ui::button("Ghost", ButtonVariant::Ghost, Size::Compact)
                .shortcut(ui::shortcut("N"))
                .build(&t.k),
        )
        .child(ui::button("Muted", ButtonVariant::GhostMuted, Size::Compact).build(&t.k))
        .child(ui::button("Faint", ButtonVariant::GhostFaint, Size::Compact).build(&t.k))
        .child(
            ui::button("New", ButtonVariant::Outline, Size::Compact)
                .icon(&ui::PLUS)
                .trailing_icon(&ui::CHEVRON_OPEN)
                .build(&t.k),
        )
        .child(
            ui::button("Saving", ButtonVariant::Submit, Size::Compact)
                .loading(true)
                .build(&t.k),
        );

    col().w_full().gap(S6).child(primary).child(ghost)
}

fn button_sizes_and_disabled(t: &Theme) -> El {
    let sizes = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(ui::button("Micro", ButtonVariant::Outline, Size::Micro).build(&t.k))
        .child(ui::button("Small", ButtonVariant::Outline, Size::Small).build(&t.k))
        .child(ui::button("Compact", ButtonVariant::Outline, Size::Compact).build(&t.k))
        .child(ui::button("Default", ButtonVariant::Outline, Size::Default).build(&t.k))
        .child(ui::button("Large", ButtonVariant::Outline, Size::Large).build(&t.k));

    let disabled = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            ui::button("Disabled submit", ButtonVariant::Submit, Size::Small)
                .disabled(true)
                .build(&t.k),
        )
        .child(
            ui::button("Disabled outline", ButtonVariant::Outline, Size::Small)
                .disabled(true)
                .build(&t.k),
        )
        .child(
            ui::button("Disabled ghost", ButtonVariant::Ghost, Size::Small)
                .disabled(true)
                .build(&t.k),
        );

    col().w_full().gap(S6).child(sizes).child(disabled)
}

fn icon_and_inline_buttons(t: &Theme) -> El {
    row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            ui::icon_button(
                &ui::CLOSE,
                "Tiny close",
                IconButtonVariant::GhostMuted,
                Size::Tiny,
            )
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::PLUS,
                "Micro add",
                IconButtonVariant::GhostMuted,
                Size::Micro,
            )
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::PIN,
                "Pin active",
                IconButtonVariant::Ghost,
                Size::Small,
            )
            .active(true)
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::LIST_FILTER,
                "Filter",
                IconButtonVariant::Neutral,
                Size::Compact,
            )
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::ARROW_UP,
                "Send",
                IconButtonVariant::Submit,
                Size::Default,
            )
            .circle()
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::ARROW_UP,
                "Send disabled",
                IconButtonVariant::Submit,
                Size::Default,
            )
            .circle()
            .disabled(true)
            .build(&t.k),
        )
        .child(
            ui::icon_button(
                &ui::SETTINGS,
                "Disabled",
                IconButtonVariant::Ghost,
                Size::Small,
            )
            .disabled(true)
            .build(&t.k),
        )
        .child(rune::spacer())
        .child(
            ui::inline_button("Copy")
                .icon(&ui::COPY)
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.diff_step += 1))
                .build(t),
        )
        .child(
            ui::inline_button("Worked 5s")
                .trailing_icon(&ui::CHEVRON)
                .style(InlineStyle::InkOnly)
                .build(t),
        )
        .child(
            ui::inline_button("Retry")
                .tone(Tone::Danger)
                .icon(&ui::ROTATE_CW)
                .build(t),
        )
        .child(ui::inline_button("Locked").disabled(true).build(t))
}

pub(super) fn badges_card(state: &GalleryState, t: &Theme) -> El {
    let tones_a = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(ui::badge("Neutral").tone(Tone::Neutral).build(t))
        .child(ui::badge("Accent").tone(Tone::Accent).dot().build(t))
        .child(
            ui::badge("Open")
                .tone(Tone::Success)
                .icon(&ui::PR_OPEN)
                .build(t),
        )
        .child(
            ui::badge("Failed")
                .tone(Tone::Danger)
                .icon(&ui::CIRCLE_X)
                .build(t),
        )
        .child(ui::badge("Approval").tone(Tone::Warning).dot().build(t));

    let tones_b = row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(
            ui::badge("Remote")
                .tone(Tone::Info)
                .icon(&ui::SERVER)
                .pill()
                .build(t),
        )
        .child(
            ui::badge("Steer")
                .tone(Tone::Steer)
                .icon(&ui::CORNER_UP_RIGHT)
                .build(t),
        )
        .child(
            ui::badge("Merged")
                .tone(Tone::Delegated)
                .icon(&ui::GIT_MERGE)
                .build(t),
        )
        .child(ui::badge("v2.4").size(BadgeSize::Nano).mono().build(t))
        .child(
            ui::badge("Draft")
                .tone(Tone::Neutral)
                .size(BadgeSize::Nano)
                .pill()
                .build(t),
        );

    let (add, del) = DIFF_STEPS[state.diff_step % DIFF_STEPS.len()];
    let kbd_diff = row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(ui::kbd("K", t))
        .child(ui::kbd_group(&["Shift", "↵"], t))
        .child(ui::with_tooltip(
            ui::kbd_cap("Esc", t),
            "Dismiss overlay",
            Some("Esc"),
        ))
        .child(rune::spacer())
        .child(ui::diff_stat(18, 4, t))
        .child(text("·").font(t.small).color(t.faint))
        .child(ui::diff_stat_rolling("gal-diff", add, del, t))
        .child(
            ui::button("+", ButtonVariant::Outline, Size::Micro)
                .on_press(|s: &mut Desktop| with_gallery(s, |g| g.diff_step += 1))
                .build(&t.k)
                .key("btn-gallery-diff"),
        );

    let body = col()
        .w_full()
        .gap(S8)
        .child(tones_a)
        .child(tones_b)
        .child(kbd_diff);
    gallery_card(
        "card-badges",
        "Badges, keycaps & diff stats",
        "8 tones + rolling",
        body,
        t,
    )
}

pub(super) fn controls_card(state: &GalleryState, t: &Theme) -> El {
    let search_row = row()
        .w_full()
        .gap(S6)
        .child(
            ui::search_input(&state.search_filled, "Filter threads…")
                .input_key("gallery-search-filled")
                .label("Active search")
                .style(SearchStyle::Bordered)
                .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.search_filled))
                .on_clear(|s: &mut Desktop, _tx| {
                    with_gallery(s, |g| g.search_filled.text.clear());
                })
                .build(t)
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        )
        .child(
            ui::search_input(&state.search_empty, "Disabled search")
                .trailing(ui::kbd_cap("/", t))
                .disabled(true)
                .build(t)
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        );

    let switches = row()
        .w_full()
        .gap(S8)
        .items_start()
        .child(
            ui::switch(state.switches.auto_scroll)
                .label("Auto-scroll")
                .description("Follow live turns")
                .on_toggle(|s: &mut Desktop| {
                    with_gallery(s, |g| g.switches.auto_scroll = !g.switches.auto_scroll);
                })
                .build(t)
                .key("sw-gallery-on")
                .grow(1.0)
                .shrink()
                .min_w(0.0),
        )
        .child(
            ui::switch(state.switches.plan_mode)
                .label("Plan mode")
                .on_toggle(|s: &mut Desktop| {
                    with_gallery(s, |g| g.switches.plan_mode = !g.switches.plan_mode);
                })
                .build(t)
                .key("sw-gallery-off"),
        )
        .child(ui::switch(true).label("Locked").disabled(true).build(t));

    let seg_row = row().w_full().gap(S8).center_y().child(
        ui::segmented(&["Local", "Worktree", "Remote"], Some(state.segmented_mode))
            .label("Checkout mode")
            .disable_segment(2)
            .on_pick(|s: &mut Desktop, idx, _tx| with_gallery(s, |g| g.segmented_mode = idx))
            .build(t)
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
            ui::field(
                "Project name",
                ui::text_field(&state.field_ok, "Project name", "e.g. viz", Size::Compact)
                    .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.field_ok))
                    .build(&t.k)
                    .key("gallery-field-ok"),
            )
            .required()
            .description("Shown in sidebar and headers.")
            .trailing(
                ui::badge("Git")
                    .size(BadgeSize::Nano)
                    .tone(Tone::Success)
                    .build(t),
            )
            .build(t)
            .grow(1.0)
            .shrink()
            .min_w(0.0),
        )
        .child(
            ui::field(
                "Repository path",
                ui::text_field(
                    &state.field_err,
                    "Repository path",
                    "/path/to/repo",
                    Size::Compact,
                )
                .invalid(true)
                .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.field_err))
                .build(&t.k)
                .key("gallery-field-err"),
            )
            .error("Directory does not exist on this environment.")
            .build(t)
            .grow(1.0)
            .shrink()
            .min_w(0.0),
        )
}
