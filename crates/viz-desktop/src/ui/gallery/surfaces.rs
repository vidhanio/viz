//! List rows, section headers, combobox shell, tucked drawer, cards, and
//! feedback states column for the `components` gallery.

use rune::{El, col, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant};

use super::{GalleryState, edit_gallery, gallery_card, with_gallery};
use crate::{
    app::Desktop,
    theme::{HAIRLINE, RADIUS_CARD, S4, S6, S8, Size, Theme},
    ui::{self, Card, LoadingDelay, RowDensity, Tone},
};

pub(super) fn rows_card(state: &GalleryState, t: &Theme) -> El {
    let add_btn = rune_kit::IconButton::new(&ui::PLUS)
        .help("New thread")
        .variant(IconButtonVariant::GhostMuted)
        .control_size(Size::Micro.kit())
        .build(&t.k);
    let sec_open = rune_kit::SidebarHeader::new("Pinned")
        .collapsed(!state.sections.pinned_open)
        .count(3)
        .action(add_btn)
        .on_toggle(|s: &mut Desktop, _| {
            with_gallery(s, |g| g.sections.pinned_open = !g.sections.pinned_open);
        })
        .build(&t.k)
        .key("gal-sec-open");
    let sec_closed = rune_kit::SidebarHeader::new("Snoozed")
        .collapsed(!state.sections.snoozed_open)
        .count(2)
        .on_toggle(|s: &mut Desktop, _| {
            with_gallery(s, |g| g.sections.snoozed_open = !g.sections.snoozed_open);
        })
        .build(&t.k)
        .key("gal-sec-closed");

    let row_rest = ui::list_row("gal-row-0", "Compact rest row")
        .density(RowDensity::Compact)
        .leading_icon(&ui::GIT_BRANCH)
        .detail("2m")
        .selected(state.selected_row == 0)
        .on_press(|s: &mut Desktop| with_gallery(s, |g| g.selected_row = 0))
        .build(t);

    let row_selected = ui::list_row("gal-row-1", "Selected thread with monogram")
        .density(RowDensity::Default)
        .leading(ui::project_monogram("viz", 18.0, Some(0), t))
        .trailing(rune_kit::Badge::new("PR #42").build(&t.k))
        .selected(state.selected_row == 1)
        .on_press(|s: &mut Desktop| with_gallery(s, |g| g.selected_row = 1))
        .build(t);

    let row_truncated = ui::list_row(
        "gal-row-2",
        "Very long thread title that must truncate cleanly with an ellipsis before colliding with trailing chips",
    )
    .subtitle(
        "crates/viz-desktop/src/ui/row.rs — verifying min_w(0) shrink behavior across narrow columns",
    )
    .leading_icon(&ui::THREAD)
    .trailing(rune_kit::Keybind::new(&["⌘", "2"]).build(&t.k))
    .selected(state.selected_row == 2)
    .on_press(|s: &mut Desktop| with_gallery(s, |g| g.selected_row = 2))
    .build(t);

    let row_disabled = ui::list_row("gal-row-disabled", "Disabled / offline option")
        .density(RowDensity::Compact)
        .leading_icon(&ui::SERVER)
        .detail("offline")
        .disabled(true)
        .build(t);

    let body = col()
        .w_full()
        .gap(S4)
        .child(sec_open)
        .child(row_rest)
        .child(row_selected)
        .child(row_truncated)
        .child(row_disabled)
        .child(sec_closed);
    gallery_card(
        "card-rows",
        "List rows & section headers",
        "States + truncation",
        body,
        t,
    )
}

pub(super) fn surfaces_card(state: &GalleryState, t: &Theme) -> El {
    let body = col()
        .w_full()
        .gap(S8)
        .child(combobox_specimen(state, t))
        .child(card_variants_row(t))
        .child(tucked_preview(t));
    gallery_card(
        "card-surfaces",
        "Combobox, cards & tucked drawer",
        "Flat overlays",
        body,
        t,
    )
}

fn combobox_specimen(state: &GalleryState, t: &Theme) -> El {
    let query = state.combo_search.text.trim().to_ascii_lowercase();
    let entries = [
        (
            "gal-combo-hi",
            "Claude Sonnet 4.6",
            "claude",
            "Claude",
            true,
        ),
        ("gal-combo-2", "GPT-5.3 Codex", "codex", "Codex", false),
        (
            "gal-combo-3",
            "Gemini 2.5 Pro",
            "antigravity",
            "Antigravity",
            false,
        ),
    ];
    let items: Vec<El> = entries
        .into_iter()
        .filter(|(_, name, _, _, _)| query.is_empty() || name.to_ascii_lowercase().contains(&query))
        .enumerate()
        .map(|(idx, (key, title, harness_id, harness_name, default))| {
            let mut item = ui::list_row(key, title)
                .density(RowDensity::Compact)
                .leading(ui::harness_glyph(
                    harness_id,
                    harness_name,
                    14.0,
                    idx == 0,
                    t,
                ))
                .highlighted(idx == 0)
                .focus_ring_for("gallery-combo-search");
            if default {
                item = item.trailing(
                    rune_kit::Badge::new("Default")
                        .variant(rune_kit::BadgeVariant::Accent)
                        .build(&t.k),
                );
            } else {
                item = item.detail("ready");
            }
            item.build(t)
        })
        .collect();

    ui::combobox_shell("gal-combo")
        .fill_width()
        .max_height(132.0)
        .search(
            rune_kit::TextInput::new(&state.combo_search)
                .placeholder("Search models or commands…")
                .leading_icon(&ui::SEARCH)
                .width(None)
                .plain()
                .active_descendant(Some("gal-combo-hi"))
                .on_edit(|s: &mut Desktop| edit_gallery(s, |g| &mut g.combo_search))
                .build(&t.k)
                .key("gallery-combo-search"),
        )
        .items(items)
        .empty(text("No matching models").font(t.small).color(t.muted))
        .footer(
            row()
                .w_full()
                .center_y()
                .gap(S6)
                .child(text("↑↓ navigate").font(t.micro).color(t.faint))
                .child(rune::spacer())
                .child(rune_kit::Keybind::new(&["↵"]).build(&t.k)),
        )
        .build(t)
}

fn card_variants_row(t: &Theme) -> El {
    let sample = |card: Card, name: &str, muted: bool| {
        card.build(t)
            .child(
                text(name)
                    .font(t.small_medium)
                    .color(if muted { t.muted } else { t.text }),
            )
            .grow(1.0)
            .shrink()
            .min_w(0.0)
    };
    row()
        .w_full()
        .gap(S6)
        .child(sample(Card::Outline, "Outline", false))
        .child(sample(Card::Message, "Message", false))
        .child(sample(Card::Recessed, "Recessed", false))
        .child(sample(Card::Dashed, "Dashed", true))
}

fn tucked_preview(t: &Theme) -> El {
    let drawer_body = row()
        .w_full()
        .px(S8)
        .py(S6)
        .gap(S6)
        .center_y()
        .child(
            rune_kit::Badge::new("@file")
                .variant(rune_kit::BadgeVariant::Subtle)
                .build(&t.k),
        )
        .child(
            text("crates/viz-desktop/src/ui/surface.rs")
                .font(t.mono_small)
                .color(t.text)
                .truncate(),
        );
    col()
        .w_full()
        .child(ui::tucked_drawer("gal-tucked", 0, drawer_body, t))
        .child(
            row()
                .paint_order(1)
                .w_full()
                .px(S8)
                .py(S6)
                .radius(RADIUS_CARD)
                .fill(t.surface)
                .border(HAIRLINE, t.border_control)
                .child(
                    text("Tucked drawer behind composer hairline")
                        .font(t.small)
                        .color(t.muted),
                ),
        )
}

fn feedback_cards(state: &GalleryState, t: &Theme) -> Vec<El> {
    let mut cards = Vec::new();
    if state.sections.banner_visible {
        cards.push(
            rune_kit::Card::new()
                .accent(Tone::Danger.color(t))
                .title("Connection interrupted", Some(Tone::Danger.glyph()))
                .description("Reconnecting to local viz-server on 127.0.0.1:7420.")
                .action(
                    rune_kit::Button::new("Retry")
                        .variant(ButtonVariant::Danger)
                        .control_size(Size::Micro.kit())
                        .icon(&ui::ROTATE_CW)
                        .build(&t.k),
                )
                .action(
                    rune_kit::IconButton::new(&ui::CLOSE)
                        .help("Dismiss")
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Micro.kit())
                        .on_press_tx(|s: &mut Desktop, _tx| {
                            with_gallery(s, |g| g.sections.banner_visible = false);
                        })
                        .build(&t.k),
                )
                .build(&t.k)
                .key("gal-err-banner"),
        );
    }
    cards.push(
        rune_kit::Card::new()
            .accent(Tone::Warning.color(t))
            .title(
                "Approval needed before running git push",
                Some(Tone::Warning.glyph()),
            )
            .build(&t.k)
            .key("gal-warn-banner"),
    );
    cards
}

pub(super) fn feedback_card(state: &GalleryState, reduced: bool, t: &Theme) -> El {
    let stack = col()
        .w_full()
        .gap(S8)
        .children(feedback_cards(state, t))
        .child(
            row()
                .w_full()
                .gap(S8)
                .items_start()
                .child(
                    Card::Recessed
                        .build(t)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .items_center()
                        .child(
                            ui::empty_state("No matching threads")
                                .icon(&ui::INBOX)
                                .detail("Clear the search filter or start a new thread.")
                                .compact()
                                .secondary_action(
                                    rune_kit::Button::new("Clear")
                                        .variant(ButtonVariant::Ghost)
                                        .control_size(Size::Small.kit())
                                        .build(&t.k),
                                )
                                .action(
                                    rune_kit::Button::new("New thread")
                                        .variant(ButtonVariant::Outline)
                                        .control_size(Size::Small.kit())
                                        .build(&t.k),
                                )
                                .build(t),
                        ),
                )
                .child(
                    col()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .gap(S8)
                        .child(ui::loading_state(
                            if reduced {
                                "Loading (reduced motion)"
                            } else {
                                "Loading workspace…"
                            },
                            Some("Linear shimmer freezes under Reduce Motion"),
                            LoadingDelay::Immediate,
                            t,
                        ))
                        .child(ui::skeleton_rows(
                            "gal-skel",
                            3,
                            18.0,
                            state.switches.shimmer,
                            t,
                        )),
                ),
        );
    gallery_card(
        "card-feedback",
        "Empty, error, loading & skeletons",
        "Live + reduced",
        stack,
        t,
    )
}
