//! Interactive motion workbench scene exercising every primitive in
//! [`crate::motion`] with realistic `viz-desktop` content for headless video,
//! frame, and trace proofs.

mod cards;
mod panels;
mod shared;

use rune::{El, col, row, text};
use rune_kit::{
    ButtonVariant,
    shell::{PaneState, SplitView, Toolbar},
};

use self::{
    cards::{disclosure_card, reorder_card},
    panels::{diff_inspector, shimmer_presence_card, undo_toast},
    shared::shared_morph_roll_card,
};
use crate::{
    app::Desktop,
    motion::{self, DragOrder, Switch},
    theme::{CARD_INSET, HAIRLINE, RADIUS_CARD, S6, S8, S12, Size, Theme},
    ui,
};

/// State backing the deterministic `motion`, `motion-reduced`, `components`,
/// and `components-reduced` scenes.
#[derive(Debug)]
pub enum MotionDemo {
    /// Interactive motion workbench exercising every [`crate::motion`]
    /// primitive.
    Workbench(Box<WorkbenchDemo>),
    /// Interactive UI component gallery exercising every [`crate::theme`] token
    /// and [`crate::ui`] component.
    Components(Box<ui::gallery::GalleryState>),
}

impl Default for MotionDemo {
    fn default() -> Self {
        Self::Workbench(Box::default())
    }
}

impl MotionDemo {
    /// Creates a fresh state for the `components` UI gallery scene.
    pub fn components() -> Self {
        Self::Components(Box::default())
    }
}

/// State backing the `motion` and `motion-reduced` workbench scenes.
#[derive(Debug)]
pub struct WorkbenchDemo {
    fold: FoldState,
    threads: Vec<DemoThread>,
    drag: DragOrder,
    shared: SharedState,
    cta_mode: CtaMode,
    diff_step: usize,
    diff_mode: usize,
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
}

impl Default for LiveState {
    fn default() -> Self {
        Self { shimmer: true }
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

impl Default for WorkbenchDemo {
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
            diff_mode: 0,
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

fn with_demo(s: &mut Desktop, f: impl FnOnce(&mut WorkbenchDemo)) {
    if let Some(MotionDemo::Workbench(demo)) = s.motion_demo.as_mut() {
        f(demo);
    }
}

/// Renders either the motion workbench or the UI component gallery inside viz's
/// flat base16 theme.
pub fn view(demo: &MotionDemo, reduced: bool, t: &Theme) -> El {
    match demo {
        MotionDemo::Workbench(workbench) => workbench_view(workbench, reduced, t),
        MotionDemo::Components(gallery) => ui::gallery::view(gallery, reduced, t),
    }
}

fn workbench_view(demo: &WorkbenchDemo, reduced: bool, t: &Theme) -> El {
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

    let content = col()
        .key("workbench-grid")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(S12)
        .gap(S12)
        .child(top_row)
        .child(bottom_row);

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
            |s: &mut Desktop| match s.motion_demo.as_mut() {
                Some(MotionDemo::Workbench(demo)) => &mut demo.panel,
                _ => unreachable!("workbench inspector toggled outside workbench"),
            },
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

fn workbench_toolbar(demo: &WorkbenchDemo, reduced: bool, t: &Theme) -> Toolbar {
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
