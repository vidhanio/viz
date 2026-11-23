//! Shared-element, icon-morph, rolling-counter, and view-stack workbench card.

use rune::{Color, El, col, el, icon, row, text};
use rune_kit::ButtonVariant;

use super::{CtaMode, DIFF_STEPS, WorkbenchDemo, card, with_demo};
use crate::{
    app::Desktop,
    motion::{self, Switch},
    theme::{
        HAIRLINE, ICON_SMALL, RADIUS_CARD, RADIUS_CONTROL, RADIUS_INNER, S4, S6, S8, S12, Size,
        Theme,
    },
    ui,
};

const PROVIDERS: [(&str, &rune::Icon); 3] = [
    ("Claude", &ui::CLAUDE),
    ("Codex", &ui::CODEX),
    ("omp", &ui::OMP),
];

pub(super) fn shared_morph_roll_card(demo: &WorkbenchDemo, t: &Theme) -> El {
    let right_col = col()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(S8)
        .child(morph_controls(demo, t))
        .child(odometer_box(demo.diff_step, t))
        .child(shared_shell(demo.shared.expanded, t))
        .child(view_stack_preview(demo.view_depth, demo.view_switch, t));

    let body = row()
        .w_full()
        .gap(S12)
        .items_start()
        .child(provider_rail(demo.shared.rail_index, t))
        .child(right_col);

    card(
        "card-shared-morph",
        "Shared element, icon morph & number roll",
        "matched rail pill + dissolve icon swap + zero-bounce RollingText",
        None,
        body,
        t,
    )
}

fn provider_rail(selected: usize, t: &Theme) -> El {
    let mut rail = col()
        .key("provider-rail")
        .gap(S4)
        .padding(S4)
        .radius(RADIUS_CONTROL)
        .fill(t.k.c.page)
        .border(HAIRLINE, t.hairline);

    for (index, (name, glyph)) in PROVIDERS.iter().enumerate() {
        let active = selected == index;
        let pill = active.then(|| {
            motion::shared_pill(
                "demo-rail-pill",
                el().key("rail-active-bar")
                    .w(3.0)
                    .h(18.0)
                    .radius(1.5)
                    .fill(t.accent),
            )
        });
        let tab = motion::pressable(
            row()
                .key(format!("rail-{index}"))
                .role("tab")
                .label(*name)
                .h(Size::Small.height())
                .px(S6)
                .gap(S6)
                .center_y()
                .radius(RADIUS_INNER)
                .fill(if active {
                    t.selected
                } else {
                    Color::TRANSPARENT
                })
                .focusable(true)
                .on_press(move |s: &mut Desktop| {
                    with_demo(s, |d| d.shared.rail_index = index);
                })
                .child(el().w(3.0).h(18.0).center().children(pill))
                .child(
                    icon(glyph, ICON_SMALL)
                        .color(if active { t.text } else { t.muted })
                        .a11y_hidden(),
                )
                .child(
                    text(*name)
                        .font(if active { t.small_medium } else { t.small })
                        .color(if active { t.text } else { t.muted }),
                ),
        );
        rail = rail.child(tab);
    }
    rail
}

fn cta_morph_button(mode: CtaMode, t: &Theme) -> El {
    let (cta_key, cta_glyph, cta_fill, cta_fg) = match mode {
        CtaMode::Send => ("send", &ui::ARROW_UP, t.accent, t.on_accent),
        CtaMode::Stop => ("stop", &ui::STOP, t.danger, t.on_danger),
        CtaMode::Steer => ("steer", &ui::CORNER_UP_RIGHT, t.steer, t.on_accent),
    };
    motion::pressable(
        row()
            .key("btn-cta-morph")
            .role("button")
            .label("Cycle send mode")
            .size(32.0, 32.0)
            .center()
            .radius(16.0)
            .fill(cta_fill)
            .motion(motion::QUICK)
            .focusable(true)
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| {
                    d.cta_mode = match d.cta_mode {
                        CtaMode::Send => CtaMode::Stop,
                        CtaMode::Stop => CtaMode::Steer,
                        CtaMode::Steer => CtaMode::Send,
                    };
                });
            })
            .child(motion::icon_morph(
                "cta-morph-slot",
                cta_key,
                cta_glyph,
                ICON_SMALL,
                16.0,
                cta_fg,
            )),
    )
}

fn morph_controls(demo: &WorkbenchDemo, t: &Theme) -> El {
    let cta_btn = cta_morph_button(demo.cta_mode, t);
    let copy_btn = motion::pressable(
        row()
            .key("btn-copy-morph")
            .role("button")
            .label("Toggle copy state")
            .h(Size::Small.height())
            .px(S8)
            .gap(S6)
            .center_y()
            .radius(RADIUS_CONTROL)
            .fill(t.k.c.page)
            .border(HAIRLINE, t.hairline)
            .focusable(true)
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.shared.copied = !d.shared.copied);
            })
            .child(motion::icon_morph(
                "copy-morph-slot",
                if demo.shared.copied { "copied" } else { "copy" },
                if demo.shared.copied {
                    &ui::CHECK
                } else {
                    &ui::COPY
                },
                ICON_SMALL,
                14.0,
                if demo.shared.copied {
                    t.success
                } else {
                    t.muted
                },
            ))
            .child(
                text(if demo.shared.copied { "Copied" } else { "Copy" })
                    .font(t.small)
                    .color(if demo.shared.copied {
                        t.success
                    } else {
                        t.text
                    }),
            ),
    );
    row()
        .gap(S8)
        .center_y()
        .child(cta_btn)
        .child(copy_btn)
        .child(
            ui::button("Morph shell", ButtonVariant::Outline, Size::Small)
                .on_press(|s: &mut Desktop| {
                    with_demo(s, |d| d.shared.expanded = !d.shared.expanded);
                })
                .build(&t.k)
                .key("btn-shared-card"),
        )
        .child(
            ui::button("Stack", ButtonVariant::Outline, Size::Small)
                .on_press(|s: &mut Desktop| {
                    with_demo(s, |d| {
                        if d.view_depth == 0 {
                            d.view_depth = 1;
                            d.view_switch = Switch::Into;
                        } else {
                            d.view_depth = 0;
                            d.view_switch = Switch::Out;
                        }
                    });
                })
                .build(&t.k)
                .key("btn-view-stack"),
        )
}

fn odometer_box(diff_step: usize, t: &Theme) -> El {
    let (added, removed, secs) = DIFF_STEPS[diff_step % DIFF_STEPS.len()];
    col()
        .gap(S6)
        .padding(S8)
        .radius(RADIUS_CONTROL)
        .fill(t.k.c.page)
        .border(HAIRLINE, t.hairline)
        .child(
            row()
                .w_full()
                .gap(S8)
                .center_y()
                .child(text("Diff & timer").font(t.small).color(t.muted))
                .child(rune::spacer())
                .child(motion::rolling_diff_stat("demo-diff", added, removed, t))
                .child(text("·").font(t.small).color(t.faint).a11y_hidden())
                .child(motion::rolling_count(
                    "demo-secs",
                    format!("{secs}s"),
                    f64::from(secs),
                    t.mono_small,
                    t.text,
                    true,
                    &t.k,
                )),
        )
        .child(
            row()
                .gap(S6)
                .child(
                    ui::button("Step +", ButtonVariant::Outline, Size::Small)
                        .on_press(|s: &mut Desktop| {
                            with_demo(s, |d| d.diff_step = (d.diff_step + 1) % DIFF_STEPS.len());
                        })
                        .build(&t.k)
                        .key("btn-roll-up"),
                )
                .child(
                    ui::button("Step −", ButtonVariant::Outline, Size::Small)
                        .on_press(|s: &mut Desktop| {
                            with_demo(s, |d| {
                                d.diff_step =
                                    (d.diff_step + DIFF_STEPS.len() - 1) % DIFF_STEPS.len();
                            });
                        })
                        .build(&t.k)
                        .key("btn-roll-down"),
                ),
        )
}

fn shared_shell(expanded: bool, t: &Theme) -> El {
    if expanded {
        motion::shared_surface(
            "demo-composer-shell",
            col()
                .key("shared-expanded")
                .w_full()
                .padding(S8)
                .gap(S4)
                .radius(RADIUS_CARD)
                .fill(t.k.c.page)
                .border(HAIRLINE, t.accent)
                .child(
                    row()
                        .w_full()
                        .center_y()
                        .child(
                            text("Docked composer surface")
                                .font(t.small_medium)
                                .color(t.text),
                        )
                        .child(rune::spacer())
                        .child(text("matched_crop").font(t.mono_micro).color(t.accent)),
                )
                .child(
                    text("Surface changes aspect cleanly without distorting text.")
                        .font(t.small)
                        .color(t.muted),
                ),
        )
    } else {
        motion::shared_surface(
            "demo-composer-shell",
            row()
                .key("shared-compact")
                .px(S8)
                .py(S6)
                .gap(S6)
                .center_y()
                .radius(RADIUS_CONTROL)
                .fill(t.k.c.page)
                .border(HAIRLINE, t.hairline)
                .child(text("Compact hero pill").font(t.small).color(t.muted)),
        )
    }
}

fn view_stack_preview(depth: usize, switch: Switch, t: &Theme) -> El {
    row()
        .key("view-stack-clip")
        .w_full()
        .h(26.0)
        .center_y()
        .clip_rounded(0.0)
        .child_exit(switch.view_exit(motion::TRAVEL_TITLE))
        .child(
            row()
                .key(format!("stack-level-{depth}"))
                .w_full()
                .gap(S6)
                .center_y()
                .transition(switch.view_enter(motion::TRAVEL_TITLE))
                .child(
                    text(if depth == 0 {
                        "Palette: Actions"
                    } else {
                        "Palette / Switch project"
                    })
                    .font(t.small_medium)
                    .color(t.text),
                ),
        )
}
