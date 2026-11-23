//! Disclosure and reorder workbench cards.

use std::f32::consts::FRAC_PI_2;

use rune::{El, ScrollSpec, col, icon, row, text};
use rune_kit::{ButtonVariant, ScrollBar, ScrollBarStyle};

use super::{DemoThread, MotionDemo, card, with_demo};
use crate::{
    app::Desktop,
    motion::{self, DragOrder},
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CARD, RADIUS_CONTROL, S6, S8, S12, S16, Size, Theme},
    ui,
};

pub(super) fn disclosure_card(demo: &MotionDemo, t: &Theme) -> El {
    let mut inner = motion::fold_container(col().key("fold-timeline-inner").w_full().gap(S8))
        .child(user_bubble("Audit motion tokens across viz-desktop", t))
        .child(fold_toggle_row(demo.fold.open, t));

    if demo.fold.open {
        inner = inner.child(motion::fold_row(fold_steps(t)));
    }
    if demo.launched_messages > 0 {
        inner = inner.child(
            row()
                .key(format!("launched-row-{}", demo.launched_messages))
                .w_full()
                .justify_end()
                .child(
                    col()
                        .key(format!("launched-msg-{}", demo.launched_messages))
                        .px(S12)
                        .py(S6)
                        .radius(RADIUS_CARD)
                        .fill(t.mine)
                        .transition(motion::row_enter(motion::TRAVEL_SMALL))
                        .child(
                            text("Follow-up launched from composer")
                                .font(t.small)
                                .color(t.text),
                        ),
                ),
        );
    }
    inner = inner.child(
        text(
            "All springs use zero bounce so rapid reversals keep continuous velocity without \
             shifting the scroll anchor.",
        )
        .key("fold-footer-prose")
        .font(t.small)
        .color(t.text)
        .wrap(),
    );

    let scroll = col()
        .key("fold-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_anchor()
        .scroll_edge_fade(S12)
        .child(inner)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k));

    let launch_btn = ui::button("Launch msg", ButtonVariant::GhostMuted, Size::Small)
        .on_press_tx(|s: &mut Desktop, tx| {
            let next = s
                .motion_demo
                .as_ref()
                .map_or(1, |d| d.launched_messages + 1);
            motion::launch_from(tx, "btn-launch", format!("launched-msg-{next}"));
            with_demo(s, |d| d.launched_messages = next);
        })
        .build(&t.k)
        .key("btn-launch");

    card(
        "card-disclosure",
        "Height disclosure & scroll anchor",
        "fold_glide + fold_feather + keep_on_screen + launch",
        Some(launch_btn),
        scroll,
        t,
    )
}

fn user_bubble(label: &str, t: &Theme) -> El {
    row().key("fold-user-bubble").w_full().justify_end().child(
        col()
            .px(S12)
            .py(S8)
            .radius(RADIUS_CARD)
            .fill(t.mine)
            .child(text(label).font(t.small).color(t.text)),
    )
}

fn fold_toggle_row(open: bool, t: &Theme) -> El {
    row()
        .key("fold-header")
        .role("button")
        .label("Toggle worked summary")
        .expanded(open)
        .w_full()
        .h(Size::Small.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .hover_group()
        .focusable(true)
        .on_press_tx(|s: &mut Desktop, tx| {
            motion::toggle_anchored("fold-header", tx);
            with_demo(s, |d| d.fold.open = !d.fold.open);
        })
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text("Worked for 5s · 3 tool calls")
                .font(t.small_medium)
                .color(t.muted)
                .group_color(t.text),
        )
}

fn fold_steps(t: &Theme) -> El {
    col()
        .key("fold-body")
        .w_full()
        .pl(S16)
        .gap(S6)
        .child(tool_line(
            &ui::FILE_READ,
            "Read crates/viz-desktop/src/theme.rs",
            "811 lines",
            t,
        ))
        .child(tool_line(
            &ui::FILE_EDIT,
            "Edit crates/viz-desktop/src/motion.rs",
            "+148 -12",
            t,
        ))
        .child(tool_line(
            &ui::TERMINAL,
            "Run cargo clippy --workspace",
            "0 warnings",
            t,
        ))
}

fn tool_line(glyph: &'static rune::Icon, label: &str, detail: &str, t: &Theme) -> El {
    row()
        .w_full()
        .gap(S6)
        .center_y()
        .child(icon(glyph, ICON_SMALL).color(t.faint).a11y_hidden())
        .child(
            text(label)
                .font(t.mono_small)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
        .child(text(detail).font(t.mono_micro).color(t.faint))
}

pub(super) fn reorder_card(demo: &MotionDemo, t: &Theme) -> El {
    let actions = row()
        .gap(S6)
        .center_y()
        .child(
            ui::button("Rotate FLIP", ButtonVariant::Outline, Size::Small)
                .on_press(|s: &mut Desktop| {
                    with_demo(s, |d| {
                        if d.threads.len() > 1 {
                            let last = d.threads.pop().expect("non-empty");
                            d.fold.promoted = Some(last.id);
                            d.threads.insert(0, last);
                        }
                    });
                })
                .build(&t.k)
                .key("btn-reorder"),
        )
        .child(
            ui::button(
                if demo.fold.extra_row {
                    "Remove row"
                } else {
                    "Insert row"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.fold.extra_row = !d.fold.extra_row);
            })
            .build(&t.k)
            .key("btn-row-toggle"),
        );

    let count = demo.threads.len();
    let mut list = motion::reflow_container(col().key("drag-list").w_full().gap(S6));
    for (index, item) in demo.threads.iter().enumerate() {
        let mut row_el = draggable_thread_row(item, demo.drag, index, count, t);
        if demo.fold.promoted == Some(item.id) && !demo.drag.is_lifted(index) {
            row_el = row_el.raise();
        }
        list = list.child(row_el);
    }
    if demo.fold.extra_row {
        list = list.child(motion::reflow(
            row()
                .key("t-extra")
                .w_full()
                .px(S8)
                .py(S6)
                .gap(S8)
                .center_y()
                .radius(RADIUS_CONTROL)
                .fill(t.selected)
                .border(HAIRLINE, t.accent)
                .transition(motion::row_enter(motion::TRAVEL_SMALL))
                .child(icon(&ui::PLUS, ICON_SMALL).color(t.accent).a11y_hidden())
                .child(
                    text("Newly spawned thread")
                        .font(t.small_medium)
                        .color(t.text),
                ),
        ));
    }

    card(
        "card-reorder",
        "List reorder (FLIP) & drag settle",
        "Drag any row vertically or rotate to verify velocity-preserving reflow",
        Some(actions),
        list,
        t,
    )
}

fn draggable_thread_row(
    item: &DemoThread,
    drag: DragOrder,
    index: usize,
    count: usize,
    t: &Theme,
) -> El {
    let base = row()
        .key(item.id)
        .w_full()
        .px(S8)
        .py(S6)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(t.k.c.page)
        .border(HAIRLINE, t.hairline)
        .child(
            icon(if item.pinned { &ui::PIN } else { &ui::THREAD }, ICON_SMALL)
                .color(if item.pinned { t.accent } else { t.faint })
                .a11y_hidden(),
        )
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(
                    text(item.title)
                        .font(t.small_medium)
                        .color(t.text)
                        .truncate(),
                )
                .child(
                    row()
                        .gap(S6)
                        .center_y()
                        .child(text(item.project).font(t.mono_micro).color(t.muted))
                        .child(icon(&ui::GIT_BRANCH, 10.0).color(t.faint).a11y_hidden())
                        .child(text(item.branch).font(t.mono_micro).color(t.faint)),
                ),
        )
        .child(text(item.age).font(t.mono_micro).color(t.faint));

    motion::drag_row(
        base,
        drag,
        index,
        count,
        t.surface,
        t.accent,
        move |s: &mut Desktop, gesture, _tx| {
            with_demo(s, |d| {
                let len = d.threads.len();
                if let Some((from, to)) = d.drag.handle(index, len, gesture) {
                    let moved = d.threads.remove(from);
                    d.threads.insert(to, moved);
                }
            });
        },
    )
}
