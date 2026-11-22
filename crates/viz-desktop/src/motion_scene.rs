//! Interactive motion workbench scene exercising every primitive in
//! [`crate::motion`] with realistic `viz-desktop` content for headless video,
//! frame, and trace proofs.

use std::f32::consts::FRAC_PI_2;

use rune::{Color, Edge, El, ScrollSpec, col, el, icon, row, text};
use rune_kit::{
    ButtonVariant, IconButtonVariant,
    shell::{PaneState, SplitView, Toolbar},
};

use crate::{
    app::Desktop,
    motion::{self, DragOrder, Switch},
    theme::{
        CARD_INSET, HAIRLINE, ICON, ICON_SMALL, RADIUS_CARD, RADIUS_CONTROL, RADIUS_INNER, S2, S4,
        S6, S8, S12, S16, Size, TOOLBAR_HEIGHT, Theme,
    },
    ui,
};

/// State backing the deterministic `motion` and `motion-reduced` scenes.
#[derive(Debug)]
pub struct MotionDemo {
    fold: FoldState,
    threads: Vec<DemoThread>,
    drag: DragOrder,
    shared: SharedState,
    cta_mode: CtaMode,
    diff_step: usize,
    live: LiveState,
    overlays: OverlayState,
    panel: PaneState,
    view_depth: usize,
    view_switch: Switch,
    launched_messages: usize,
}

#[derive(Debug, Default)]
struct FoldState {
    open: bool,
    extra_row: bool,
    promoted: Option<&'static str>,
}

#[derive(Debug, Default)]
struct SharedState {
    rail_index: usize,
    expanded: bool,
    copied: bool,
}

#[derive(Debug)]
struct LiveState {
    shimmer: bool,
    terminal: bool,
}

impl Default for LiveState {
    fn default() -> Self {
        Self {
            shimmer: true,
            terminal: false,
        }
    }
}

#[derive(Debug, Default)]
struct OverlayState {
    drawer: bool,
    popover: bool,
    toast: bool,
}

#[derive(Debug, Clone)]
struct DemoThread {
    id: &'static str,
    title: &'static str,
    project: &'static str,
    branch: &'static str,
    age: &'static str,
    pinned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtaMode {
    Send,
    Stop,
    Steer,
}

impl Default for MotionDemo {
    fn default() -> Self {
        Self {
            fold: FoldState::default(),
            threads: vec![
                DemoThread {
                    id: "t-export",
                    title: "Export markdown notes",
                    project: "workbench",
                    branch: "feat/export",
                    age: "2m",
                    pinned: true,
                },
                DemoThread {
                    id: "t-trim",
                    title: "Trim trailing whitespace",
                    project: "workbench",
                    branch: "fix/trim",
                    age: "14m",
                    pinned: false,
                },
                DemoThread {
                    id: "t-audit",
                    title: "Audit error banners",
                    project: "field-notes",
                    branch: "main",
                    age: "1h",
                    pinned: false,
                },
                DemoThread {
                    id: "t-release",
                    title: "Draft release notes",
                    project: "field-notes",
                    branch: "docs/v2",
                    age: "3h",
                    pinned: false,
                },
            ],
            drag: DragOrder::new().gap(S6),
            shared: SharedState::default(),
            cta_mode: CtaMode::Send,
            diff_step: 0,
            live: LiveState::default(),
            overlays: OverlayState::default(),
            panel: PaneState::new(300.0).range(240.0, 420.0).collapsed(),
            view_depth: 0,
            view_switch: Switch::Cut,
            launched_messages: 0,
        }
    }
}

const DIFF_STEPS: [(u32, u32, u32); 4] = [(5, 1, 9), (42, 13, 10), (118, 27, 14), (19, 4, 6)];

const PROVIDERS: [(&str, &rune::Icon); 3] = [
    ("Claude", &ui::CLAUDE),
    ("Codex", &ui::CODEX),
    ("omp", &ui::OMP),
];

const DRAWER_FILES: [(&str, &str); 4] = [
    ("crates/viz-desktop/src/motion.rs", "tokens + primitives"),
    ("crates/viz-desktop/src/theme.rs", "base16 kit alignment"),
    ("crates/viz-desktop/src/timeline.rs", "fold + anchoring"),
    ("crates/viz-desktop/src/header.rs", "title switch"),
];

fn with_demo(s: &mut Desktop, f: impl FnOnce(&mut MotionDemo)) {
    if let Some(demo) = s.motion_demo.as_mut() {
        f(demo);
    }
}

/// Renders the motion workbench inside viz's flat base16 theme.
pub fn view(demo: &MotionDemo, reduced: bool, t: &Theme) -> El {
    let mut overlays = Vec::new();
    let top_row = row()
        .key("top-row")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(S12)
        .child(disclosure_card(demo, t))
        .child(reorder_card(demo, t));

    let bottom_row = row()
        .key("bottom-row")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(S12)
        .child(shared_morph_roll_card(demo, t))
        .child(shimmer_presence_card(demo, t, &mut overlays));

    let mut content = col()
        .key("workbench-grid")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(S12)
        .gap(S12)
        .child(top_row)
        .child(bottom_row);

    if demo.live.terminal {
        content = content.child(terminal_drawer(t));
    }

    let main = motion::stable_content(
        col()
            .key("workbench-main")
            .w_full()
            .h_full()
            .min_h(0.0)
            .fill(t.k.c.page)
            .child(ui::rule(t).key("workbench-header-rule"))
            .child(content),
    );

    let split = SplitView::new(main)
        .toolbar(workbench_toolbar(demo, reduced, t))
        .toolbar_in_panel()
        .inspector(
            &demo.panel,
            |s: &mut Desktop| &mut s.motion_demo.as_mut().expect("motion demo").panel,
            diff_inspector(demo, t),
        )
        .hide_sidebar_toggle()
        .frame_inset(0.0)
        .pane_motion(motion::pane_motion())
        .reduced_motion(reduced)
        .build(&t.shell);

    if demo.overlays.toast {
        overlays.push(undo_toast(t));
    }
    rune_kit::root(split, overlays).hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
}

fn workbench_toolbar(demo: &MotionDemo, reduced: bool, t: &Theme) -> Toolbar {
    let leading = row()
        .key("workbench-title")
        .gap(S8)
        .center_y()
        .child(text("Motion workbench").font(t.title).color(t.text))
        .child(text("·").font(t.small).color(t.faint).a11y_hidden())
        .child(
            text(if reduced {
                "Reduced motion"
            } else {
                "Zero-bounce springs"
            })
            .font(t.small)
            .color(if reduced { t.warning } else { t.muted }),
        );
    let trailing = row()
        .key("workbench-actions")
        .gap(S8)
        .center_y()
        .child(
            ui::button(
                if demo.live.shimmer {
                    "Live: on"
                } else {
                    "Live: off"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.live.shimmer = !d.live.shimmer);
            })
            .build(&t.k)
            .key("btn-shimmer"),
        )
        .child(
            ui::button("Toast", ButtonVariant::Outline, Size::Small)
                .on_press(|s: &mut Desktop| {
                    with_demo(s, |d| d.overlays.toast = !d.overlays.toast);
                })
                .build(&t.k)
                .key("btn-toast"),
        )
        .child(
            ui::button(
                if demo.live.terminal {
                    "Hide terminal"
                } else {
                    "Terminal"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.live.terminal = !d.live.terminal);
            })
            .build(&t.k)
            .key("btn-terminal"),
        )
        .child(
            ui::button(
                if demo.panel.is_open() {
                    "Hide diff"
                } else {
                    "Diff panel"
                },
                ButtonVariant::Outline,
                Size::Small,
            )
            .on_press(|s: &mut Desktop| {
                with_demo(s, |d| d.panel.toggle());
            })
            .build(&t.k)
            .key("btn-panel"),
        );
    Toolbar::new().leading(leading).trailing(trailing)
}

fn card(key: &str, title: &str, subtitle: &str, actions: Option<El>, body: El, t: &Theme) -> El {
    let mut head = row()
        .key(format!("{key}-head"))
        .w_full()
        .gap(S8)
        .center_y()
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(text(title).font(t.ui_medium).color(t.text).truncate())
                .child(text(subtitle).font(t.small).color(t.muted).truncate()),
        );
    if let Some(actions) = actions {
        head = head.child(actions);
    }
    col()
        .key(key)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .min_h(0.0)
        .padding(CARD_INSET)
        .gap(S12)
        .radius(RADIUS_CARD)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .child(head)
        .child(ui::rule(t))
        .child(body)
}

fn disclosure_card(demo: &MotionDemo, t: &Theme) -> El {
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
        .child(inner);

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

fn reorder_card(demo: &MotionDemo, t: &Theme) -> El {
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

fn shared_morph_roll_card(demo: &MotionDemo, t: &Theme) -> El {
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

fn morph_controls(demo: &MotionDemo, t: &Theme) -> El {
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

fn shimmer_presence_card(demo: &MotionDemo, t: &Theme, overlays: &mut Vec<El>) -> El {
    let mut drawer_host = col().key("presence-host").w_full().gap(S6);
    if demo.overlays.drawer {
        drawer_host = drawer_host.child(tucked_files_drawer(t));
    }
    if demo.overlays.popover {
        let right = if demo.panel.is_open() {
            demo.panel.width() + S16
        } else {
            S16
        };
        overlays.push(details_popover(right, t));
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
        .child(live_shimmer_row(demo.live.shimmer, t))
        .child(drawer_host)
        .child(rune::spacer())
        .child(controls);

    card(
        "card-shimmer",
        "Live shimmer & tucked presence",
        "Linear GPU sweep + staggered @files drawer + flat popover",
        None,
        body,
        t,
    )
}

fn live_shimmer_row(active: bool, t: &Theme) -> El {
    row()
        .key("live-shimmer-row")
        .w_full()
        .px(S12)
        .py(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(t.k.c.page)
        .border(HAIRLINE, if active { t.accent } else { t.hairline })
        .children(motion::shimmer_band(
            "demo-shimmer",
            active,
            t.accent,
            RADIUS_CONTROL,
        ))
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
                        "Read crates/viz-desktop/src/motion.rs"
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

fn details_popover(right: f32, t: &Theme) -> El {
    col()
        .key("demo-details-popover")
        .pin(None, Some(TOOLBAR_HEIGHT + S8), Some(right), None)
        .w(248.0)
        .padding(S12)
        .gap(S6)
        .radius(RADIUS_CARD)
        .fill(t.overlay.c.layer_01)
        .border(HAIRLINE, t.hairline)
        .transition(motion::popover(-motion::TRAVEL_MICRO))
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
            text("Flat opaque base16 overlay with 1px hairline and zero drop shadow.")
                .font(t.small)
                .color(t.muted)
                .wrap(),
        )
}

fn diff_inspector(demo: &MotionDemo, t: &Theme) -> El {
    let (added, removed, _) = DIFF_STEPS[demo.diff_step % DIFF_STEPS.len()];
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
                    text("crates/viz-desktop/src/motion.rs")
                        .font(t.small_medium)
                        .color(t.text)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .truncate(),
                )
                .child(motion::rolling_diff_stat("panel-diff", added, removed, t)),
        )
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
            text("Main pane uses hold_landing_width() so prose wraps once instead of reflowing every frame.")
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

fn terminal_drawer(t: &Theme) -> El {
    col()
        .key("demo-terminal-panel")
        .w_full()
        .h(72.0)
        .padding(S8)
        .gap(S4)
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .transition(motion::slide_panel(Edge::Bottom))
        .child(
            row()
                .gap(S6)
                .center_y()
                .child(icon(&ui::TERMINAL, ICON_SMALL).color(t.muted).a11y_hidden())
                .child(text("Terminal · bash").font(t.small_medium).color(t.text)),
        )
        .child(
            text("$ cargo clippy --workspace --all-targets -- -D warnings   [0 warnings]")
                .font(t.mono_small)
                .color(t.success),
        )
}

fn undo_toast(t: &Theme) -> El {
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
