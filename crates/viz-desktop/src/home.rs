//! Centered new-thread Home / Draft Hero screen (`home.md`):
//!
//! - **State A (No project)**: `"What should we work on?"` + quiet `"No
//!   project"` trigger opening a flat `288px` searchable Project Picker
//!   combobox (`home/project_picker.rs`).
//! - **State B (Project selected)**: `"What should we build in <project>?"`
//!   (wide single line, stacked narrow fallback) + `"or start without a
//!   project"` sub-row (`Ctrl+Alt+N` / `Cmd+Option+N`).
//! - **State C (Zero projects configured)**: `"What should we work on?"`, `"Add
//!   a project, or start without one."`, primary `"Add project"` + outline
//!   `"Start without a project"`.
//! - **Bottom Context Strip**: Environment, Git worktree mode, Branch/Base-ref,
//!   and Access Mode selectors directly beneath the composer card.

mod project_picker;

use rune::{Align, Color, El, TextStyle, Transition, col, el, row, text};
use rune_kit::ButtonVariant;
use viz_client::protocol::Project;

use crate::{
    app::Desktop,
    composer,
    model::Model,
    motion,
    theme::{
        CHAT_CONTENT_MAX_WIDTH, CHAT_LANE_PADDING_X, HAIRLINE, ICON_MD, RADIUS_SM, S2, S4, S6, S8,
        S10, S16, S20, S24, Size, Theme,
    },
    ui,
    update::{ComposerMsg, HOME_PROJECT_PICKER, Msg, SelectionMsg, SidebarMsg},
};

/// Element key for the hero project selector trigger.
pub const PROJECT_TRIGGER_KEY: &str = "draft-project-trigger";
/// Short-viewport height threshold (`34rem` = `544pt`) switching hero layout
/// from vertical center to bottom-aligned dock.
const SHORT_VIEWPORT_HEIGHT: f32 = 544.0;
/// Narrow main-pane width threshold (`640pt`) switching headline scale.
const NARROW_WIDTH: f32 = 640.0;

/// Whether the main pane should render the centered Home / Draft Hero screen.
#[must_use]
pub const fn is_active(m: &Model) -> bool {
    m.connected() && m.selection.selected.is_none()
}

/// Renders the centered Home / Draft Hero surface and mounts its project
/// picker / context strip overlays into `layer`.
pub fn view(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    if m.selection.new_thread.is_none() && !m.cache().has_projects() {
        return zero_projects_hero(t);
    }

    let sidebar_w = if s.widgets.sidebar_pane.is_open() {
        s.widgets.sidebar_pane.shown_width()
    } else {
        0.0
    };
    let main_width = (m.panels.window_width - sidebar_w).max(320.0);
    let lane_width = CHAT_LANE_PADDING_X
        .mul_add(-2.0, main_width)
        .clamp(240.0, CHAT_CONTENT_MAX_WIDTH);
    let narrow = main_width < NARROW_WIDTH;
    let short_viewport = m.panels.window_height <= SHORT_VIEWPORT_HEIGHT;

    let hero_exit = Transition::offset(0.0, -motion::TRAVEL_SMALL)
        .combined(Transition::opacity())
        .springs(motion::SETTLE_PAIR);

    let hero = match m.project() {
        Some(project) => state_b_hero(s, project, lane_width, narrow, t, layer),
        None => state_a_hero(s, narrow, t, layer),
    }
    .key("home-hero-heading")
    .transition(hero_exit);

    let dock = composer::home_composer(s, lane_width, t, layer);

    let content = col()
        .key("home-column")
        .w_full()
        .max_w(CHAT_CONTENT_MAX_WIDTH)
        .items_center()
        .child_exit(hero_exit)
        .child(hero)
        .child(dock);

    if short_viewport {
        col()
            .key("home-screen")
            .w_full()
            .h_full()
            .min_h(0.0)
            .px(CHAT_LANE_PADDING_X)
            .pb(S16)
            .items_center()
            .justify_end()
            .child(content)
    } else {
        col()
            .key("home-screen")
            .w_full()
            .h_full()
            .min_h(0.0)
            .px(CHAT_LANE_PADDING_X)
            .items_center()
            .child(el().grow(1.0).a11y_hidden())
            .child(content)
            .child(el().grow(1.08).a11y_hidden())
    }
}

/// State A: No project selected (`"What should we work on?"` + `"No project"`).
fn state_a_hero(s: &Desktop, narrow: bool, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = is_project_picker_open(s);
    let headline_font = if narrow { t.hero_narrow } else { t.hero };

    let trigger = row()
        .key(PROJECT_TRIGGER_KEY)
        .role("combobox")
        .label("Project: No project")
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .hover_group()
        .radius(RADIUS_SM)
        .px(S4)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::ToggleHomeProjectPicker), tx);
        })
        .child(
            col()
                .items_center()
                .gap(S2)
                .a11y_hidden()
                .child(
                    text("No project")
                        .font(t.ui)
                        .color(if open { t.text } else { t.muted })
                        .group_color(t.text),
                )
                .child(hover_underline(open, t.text, t.muted)),
        );

    if open {
        project_picker::popover(s, t, layer);
    }

    col()
        .w_full()
        .items_center()
        .pb(S16)
        .child(
            text("What should we work on?")
                .font(headline_font)
                .color(t.text_strong)
                .align(Align::Center),
        )
        .child(row().mt(S8).h(S24).center().child(trigger))
}

/// State B: Project selected (`"What should we build in <project>?"` +
/// `"or start without a project"`).
fn state_b_hero(
    s: &Desktop,
    project: &Project,
    lane_width: f32,
    narrow: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let open = is_project_picker_open(s);
    let approx_chars = "What should we build in ?".len() + project.name.chars().count();
    #[expect(
        clippy::cast_precision_loss,
        reason = "project title length is small and used only for single-line fit estimate"
    )]
    let estimated_w = (approx_chars as f32).mul_add(15.8, 44.0);
    let inline_fit = !narrow && estimated_w <= lane_width - 16.0;

    let heading = if inline_fit {
        state_b_inline_heading(s, project, open, t)
    } else {
        state_b_stacked_heading(s, project, lane_width, narrow, open, t)
    };

    if open {
        project_picker::popover(s, t, layer);
    }

    let without_shortcut = if cfg!(target_os = "macos") {
        "⌘⌥N"
    } else {
        "Ctrl+Alt+N"
    };
    let without = row()
        .key("start-without-project")
        .role("button")
        .label("or start without a project")
        .help(format!("Start without a project ({without_shortcut})"))
        .shortcut_hint(without_shortcut)
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .hover_group()
        .radius(RADIUS_SM)
        .px(S4)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Selection(SelectionMsg::StartWithoutProject), tx);
        })
        .child(
            col()
                .items_center()
                .gap(S2)
                .a11y_hidden()
                .child(
                    text("or start without a project")
                        .font(t.ui)
                        .color(t.faint)
                        .group_color(t.muted),
                )
                .child(hover_underline(false, t.muted, t.muted)),
        );

    col()
        .w_full()
        .items_center()
        .pb(S16)
        .child(heading)
        .child(row().mt(S8).h(S24).center().child(without))
}

fn state_b_inline_heading(s: &Desktop, project: &Project, open: bool, t: &Theme) -> El {
    let project_font = TextStyle {
        weight: 520.0,
        ..t.hero
    };
    let trigger = row()
        .key(PROJECT_TRIGGER_KEY)
        .role("combobox")
        .label(format!("Project: {}", project.name))
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .hover_group()
        .radius(RADIUS_SM)
        .px(S4)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::ToggleHomeProjectPicker), tx);
        })
        .child(
            col()
                .items_center()
                .gap(S2)
                .a11y_hidden()
                .child(
                    row()
                        .center_y()
                        .gap(S8)
                        .child(ui::project_mark(s, Some(project), 22.0, t))
                        .child(text(&project.name).font(project_font).color(t.text_strong)),
                )
                .child(hover_underline(open, t.text_strong, t.muted)),
        );
    row()
        .w_full()
        .center()
        .child(
            text("What should we build in ")
                .font(t.hero)
                .color(t.text_strong),
        )
        .child(trigger)
        .child(text("?").font(t.hero).color(t.text_strong))
}

fn state_b_stacked_heading(
    s: &Desktop,
    project: &Project,
    lane_width: f32,
    narrow: bool,
    open: bool,
    t: &Theme,
) -> El {
    let headline_font = if narrow { t.hero_narrow } else { t.hero };
    let trigger_font = TextStyle {
        size: 16.0,
        line_height: 22.0,
        weight: 520.0,
        ..t.ui_medium
    };
    let trigger = row()
        .key(PROJECT_TRIGGER_KEY)
        .role("combobox")
        .label(format!("Project: {}", project.name))
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, S2, t.accent)
        .hover_group()
        .radius(RADIUS_SM)
        .px(S6)
        .py(S2)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::ToggleHomeProjectPicker), tx);
        })
        .child(
            col()
                .items_center()
                .gap(S2)
                .a11y_hidden()
                .child(
                    row()
                        .center_y()
                        .gap(S6)
                        .child(ui::project_mark(s, Some(project), 16.0, t))
                        .child(
                            text(&project.name)
                                .font(trigger_font)
                                .color(t.text_strong)
                                .max_w(lane_width - 64.0)
                                .truncate(),
                        )
                        .child(
                            motion::chevron(&ui::CHEVRON_OPEN, ICON_MD, open, std::f32::consts::PI)
                                .color(t.muted)
                                .group_color(t.text),
                        ),
                )
                .child(hover_underline(open, t.text, t.muted)),
        );
    col()
        .w_full()
        .items_center()
        .gap(S4)
        .child(
            text("What should we build?")
                .font(headline_font)
                .color(t.text_strong)
                .align(Align::Center),
        )
        .child(trigger)
}

/// State C: Zero projects configured on first launch (`home.md` §2).
fn zero_projects_hero(t: &Theme) -> El {
    let add = ui::button("Add project", ButtonVariant::Submit, Size::Default)
        .icon(&ui::PLUS)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Palette(crate::update::PaletteMsg::OpenProject(
                    crate::model::projects::ProjectView::Sources,
                )),
                tx,
            );
        })
        .build(&t.k)
        .key("hero-add-project");

    let without = ui::button(
        "Start without a project",
        ButtonVariant::Neutral,
        Size::Default,
    )
    .icon(&ui::MESSAGE_SQUARE_DASHED)
    .on_press_tx(|s: &mut Desktop, tx| {
        s.dispatch(
            Msg::Sidebar(SidebarMsg::NewThreadIn {
                project: None,
                branch: None,
            }),
            tx,
        );
    })
    .build(&t.k)
    .key("hero-without-project");

    col()
        .key("home-zero-projects")
        .w_full()
        .h_full()
        .min_h(0.0)
        .px(CHAT_LANE_PADDING_X)
        .center()
        .child(
            col()
                .w_full()
                .max_w(420.0)
                .items_center()
                .child(
                    text("What should we work on?")
                        .font(t.hero)
                        .color(t.text_strong)
                        .align(Align::Center),
                )
                .child(
                    text("Add a project, or start without one.")
                        .font(t.ui)
                        .color(t.muted)
                        .align(Align::Center)
                        .mt(S8),
                )
                .child(row().mt(S20).gap(S10).center().child(add).child(without)),
        )
}

fn hover_underline(always_visible: bool, open_color: Color, hover_color: Color) -> El {
    let bar = el()
        .w_full()
        .h(HAIRLINE)
        .fill(if always_visible {
            open_color
        } else {
            hover_color
        })
        .a11y_hidden();
    if always_visible {
        bar
    } else {
        bar.group_opacity(0.0, 1.0)
    }
}

fn is_project_picker_open(s: &Desktop) -> bool {
    let sending = s
        .model
        .selection
        .new_thread
        .as_ref()
        .is_some_and(|draft| draft.sending);
    s.model.composer.open_picker.as_deref() == Some(HOME_PROJECT_PICKER) && !sending
}
