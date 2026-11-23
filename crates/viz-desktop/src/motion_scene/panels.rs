//! Live status, tucked drawer, popover, inspector, and toast cards.

use rune::{El, col, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Placement, SegmentedControl};

use super::{DIFF_STEPS, WorkbenchDemo, card, with_demo};
use crate::{
    app::Desktop,
    motion,
    theme::{
        HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, RADIUS_CONTROL, RADIUS_INNER, S2, S4, S6, S8, S12,
        S16, Size, Theme,
    },
    ui,
};

const DRAWER_FILES: [(&str, &str); 4] = [
    (
        "crates/viz-desktop/src/motion/mod.rs",
        "tokens + primitives",
    ),
    (
        "crates/viz-desktop/src/theme/mod.rs",
        "base16 kit alignment",
    ),
    ("crates/viz-desktop/src/timeline.rs", "fold + anchoring"),
    ("crates/viz-desktop/src/header.rs", "title switch"),
];

pub(super) fn presence_card(demo: &WorkbenchDemo, t: &Theme, overlays: &mut Vec<El>) -> El {
    let mut drawer_host = col().key("presence-host").w_full().gap(S6);
    if demo.overlays.drawer {
        drawer_host = drawer_host.child(tucked_files_drawer(t));
    }
    if demo.overlays.popover {
        overlays.push(details_popover(t));
    }

    let controls = row()
        .w_full()
        .gap(S8)
        .center_y()
        .child(
            ui::button(
                if demo.overlays.drawer {
                    "Close @files"
                } else {
                    "Open @files"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.overlays.drawer = !d.overlays.drawer);
            })
            .build(&t.k)
            .key("btn-drawer"),
        )
        .child(
            ui::button(
                if demo.overlays.popover {
                    "Close popover"
                } else {
                    "Details popover"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.overlays.popover = !d.overlays.popover);
            })
            .build(&t.k)
            .key("btn-popover"),
        );

    let body = col()
        .w_full()
        .grow(1.0)
        .gap(S8)
        .child(live_status_row(demo.live.active, t))
        .child(drawer_host)
        .child(rune::spacer())
        .child(controls);

    card(
        "card-presence",
        "Live status & tucked presence",
        "Status change, staggered @files drawer and popover",
        None,
        body,
        t,
    )
}

fn live_status_row(active: bool, t: &Theme) -> El {
    row()
        .key("live-status-row")
        .w_full()
        .px(S12)
        .py(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(t.k.c.page)
        .child(if active {
            ui::radar(ICON_SMALL, t.accent, t)
        } else {
            motion::icon_morph(
                "live-done-slot",
                "done",
                &ui::CHECK,
                ICON_SMALL,
                14.0,
                t.success,
            )
        })
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(
                    text(if active {
                        "Thinking · inspecting spring settle"
                    } else {
                        "Settled · 0 idle frames requested"
                    })
                    .font(t.small_medium)
                    .color(t.text),
                )
                .child(
                    text(if active {
                        "Read crates/viz-desktop/src/motion/mod.rs"
                    } else {
                        "Shimmer unmounted cleanly"
                    })
                    .font(t.mono_micro)
                    .color(t.muted),
                ),
        )
}

fn tucked_files_drawer(t: &Theme) -> El {
    let mut rows = col().w_full().padding(S6).gap(S4);
    for (i, (path, note)) in DRAWER_FILES.iter().enumerate() {
        rows = rows.child(
            row()
                .key(format!("drawer-file-{i}"))
                .w_full()
                .px(S6)
                .py(S4)
                .gap(S6)
                .center_y()
                .radius(RADIUS_INNER)
                .transition(motion::row_enter(motion::TRAVEL_MICRO))
                .enter_delay(motion::stagger_delay(i, DRAWER_FILES.len()))
                .child(icon(&ui::FILE, ICON_SMALL).color(t.faint).a11y_hidden())
                .child(
                    text(*path)
                        .font(t.mono_small)
                        .color(t.text)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .truncate(),
                )
                .child(text(*note).font(t.mono_micro).color(t.faint)),
        );
    }
    let chips = row().gap(S4).px(S6).pb(S6).child(
        row()
            .key("chip-1")
            .px(S6)
            .py(S2)
            .gap(S4)
            .center_y()
            .radius(RADIUS_INNER)
            .fill(t.chip_fill)
            .transition(motion::chip_enter())
            .child(icon(&ui::PAPERCLIP, 10.0).color(t.muted).a11y_hidden())
            .child(text("motion.rs").font(t.mono_micro).color(t.text)),
    );
    col()
        .key("demo-tucked-drawer")
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.overlay.c.layer_01)
        .border(HAIRLINE, t.hairline)
        .transition(motion::tucked_menu())
        .child(rows)
        .child(chips)
}

fn details_popover(t: &Theme) -> El {
    let card = col()
        .key("demo-details-popover")
        .w(248.0)
        .padding(S12)
        .gap(S6)
        .radius(RADIUS_CARD)
        .fill(t.overlay.c.layer_01)
        .border(HAIRLINE, t.hairline)
        .child(
            row()
                .w_full()
                .center_y()
                .child(text("Thread details").font(t.ui_medium).color(t.text))
                .child(rune::spacer())
                .child(
                    ui::icon_button(
                        &ui::CLOSE,
                        "Close",
                        IconButtonVariant::GhostMuted,
                        Size::Small,
                    )
                    .on_press(|s: &mut Desktop| {
                        with_demo(s, |d| d.overlays.popover = false);
                    })
                    .build(&t.overlay),
                ),
        )
        .child(ui::rule(t))
        .child(
            text("Anchored popover with viewport flip, 1px hairline, and zero drop shadow.")
                .font(t.small)
                .color(t.muted)
                .wrap(),
        );
    rune_kit::popover::anchored(&t.overlay, card, "btn-popover", Placement::TopStart, S6)
}

pub(super) fn diff_inspector(demo: &WorkbenchDemo, t: &Theme) -> El {
    let (added, removed, _) = DIFF_STEPS[demo.diff_step % DIFF_STEPS.len()];
    let mode_toggle = SegmentedControl::new(&["Unified", "Split"], Some(demo.diff_mode))
        .label("Diff layout")
        .fit()
        .on_pick(|s: &mut Desktop, idx, _tx| {
            with_demo(s, |d| d.diff_mode = idx);
        })
        .build(&t.k)
        .key("diff-mode-segmented");
    col()
        .key("diff-inspector")
        .w_full()
        .h_full()
        .padding(S12)
        .gap(S8)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .child(
            row()
                .w_full()
                .center_y()
                .gap(S8)
                .child(icon(&ui::FILE_EDIT, ICON).color(t.accent).a11y_hidden())
                .child(
                    text("crates/viz-desktop/src/motion/mod.rs")
                        .font(t.small_medium)
                        .color(t.text)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .truncate(),
                )
                .child(motion::rolling_diff_stat("panel-diff", added, removed, t)),
        )
        .child(mode_toggle)
        .child(ui::rule(t))
        .child(diff_line(
            "+ pub const SETTLE: Spring = Spring::new(0.24, 0.0);",
            true,
            t,
        ))
        .child(diff_line(
            "+ pub const GLIDE: Spring = Spring::new(0.28, 0.0);",
            true,
            t,
        ))
        .child(diff_line(
            "- const OLD_SPRING: Spring = Spring::new(0.22, 0.0);",
            false,
            t,
        ))
        .child(
            text(
                "Main pane uses hold_landing_width() so prose wraps once instead of reflowing \
                 every frame.",
            )
            .font(t.small)
            .color(t.muted)
            .wrap()
            .mt(S8),
        )
}

fn diff_line(line: &str, added: bool, t: &Theme) -> El {
    row()
        .w_full()
        .px(S6)
        .py(S2)
        .radius(RADIUS_INNER)
        .fill(if added { t.added } else { t.removed })
        .child(
            text(line)
                .font(t.mono_micro)
                .color(if added { t.success } else { t.danger })
                .truncate(),
        )
}

pub(super) fn undo_toast(t: &Theme) -> El {
    row()
        .key("demo-undo-toast")
        .pin(None, None, Some(S16), Some(S16))
        .w(300.0)
        .padding(S12)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CARD)
        .fill(t.overlay.c.layer_01)
        .border(HAIRLINE, t.hairline)
        .transition(motion::toast())
        .child(ui::dot(t.success))
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(text("Thread settled").font(t.small_medium).color(t.text))
                .child(
                    text("Export markdown notes")
                        .font(t.small)
                        .color(t.muted)
                        .truncate(),
                ),
        )
        .child(
            ui::button("Undo", ButtonVariant::Outline, Size::Small)
                .on_press(|s: &mut Desktop| {
                    with_demo(s, |d| d.overlays.toast = false);
                })
                .build(&t.overlay)
                .key("btn-toast-undo"),
        )
}
