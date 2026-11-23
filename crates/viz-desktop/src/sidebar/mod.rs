//! Unified thread-triage sidebar: titlebar chrome, search & utility bar,
//! project filter combobox, triage shelves (Drafts, Pinned, Working, Active,
//! Snoozed, Settled), hover preview cards, drag-and-drop, context menus, and
//! environments footer.

mod actions;
mod card;
mod card_meta;
mod draft;
mod filter;
mod footer;
mod hover_card;
mod menu;
mod project_mark;
mod shelves;

use rune::{Color, El, TextStyle, col, icon, row, text};
use rune_kit::IconButtonVariant;

use crate::{
    app::Desktop,
    model::{ProjectFilter, SidebarOutline},
    motion,
    theme::{
        self, HAIRLINE, ICON_MD, RADIUS_CONTROL, S2, S4, S6, S8, S12, SIDEBAR_PAD, Size,
        TOOLBAR_HEIGHT, Theme,
    },
    ui::{self, ScrollMode},
    update::{Msg, PaletteMsg, SidebarMsg},
};

/// Builds the unified triage workspace sidebar and mounts its overlays into
/// `layer`.
pub fn sidebar(s: &Desktop, titlebar_inset: f32, t: &Theme, layer: &mut Vec<El>) -> El {
    let now = theme::now_ms();
    let outline = SidebarOutline::derive(&s.model, now);

    let titlebar = titlebar_wordmark(titlebar_inset, t);
    let action_bar = utility_bar(s, now, t, layer);
    let filter_chip = active_filter_chip(s, t);
    let body = ui::scroll_viewport(
        "sidebar-scroll",
        ScrollMode::Top,
        shelves::primary_shelves(s, &outline, t, layer),
        t,
    );
    let settled = shelves::settled_drawer(s, &outline, t, layer);
    let bottom = footer::footer(s, t, layer);
    hover_card::mount(s, &outline, t, layer);

    let mut root = col()
        .key("sidebar")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(titlebar)
        .child(action_bar);
    if let Some(chip) = filter_chip {
        root = root.child(chip);
    }
    root.child(body).child(settled).child(bottom)
}

/// Mounts sidebar confirmation and rename dialogs independently of whether the
/// sidebar is visible.
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    actions::dialog(s, t, layer);
}

/// New thread icon button used in Row 2 while the sidebar is open and in the
/// main toolbar while the sidebar is collapsed.
pub fn new_thread_button(s: &Desktop, size: Size, t: &Theme) -> El {
    let m = &s.model;
    ui::icon_button(
        &ui::COMPOSE,
        "New thread",
        IconButtonVariant::GhostMuted,
        size,
    )
    .shortcut(ui::shortcut("N"))
    .disabled(!m.connected() || m.selection.new_thread.is_some())
    .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Sidebar(SidebarMsg::NewThread), tx))
    .build(&t.k)
    .key("new-thread")
}

/// Row 1 titlebar band (`viz` wordmark sitting directly beside `SplitView`'s
/// `PanelLeft` toggle button).
fn titlebar_wordmark(titlebar_inset: f32, t: &Theme) -> El {
    let lights = if titlebar_inset > 0.0 {
        titlebar_inset + S8
    } else {
        S12
    };
    let toggle_side = Size::Compact.height();
    let wordmark_left = lights + toggle_side + S8;
    let wordmark_style = TextStyle {
        size: 13.0,
        weight: 600.0,
        line_height: 18.0,
        ..t.panel_row
    };
    row()
        .key("sidebar-wordmark")
        .pin(
            Some(-SIDEBAR_PAD),
            Some(-TOOLBAR_HEIGHT),
            Some(-SIDEBAR_PAD),
            None,
        )
        .h(TOOLBAR_HEIGHT)
        .pl(wordmark_left)
        .pr(S12)
        .center_y()
        .a11y_hidden()
        .child(text("viz").font(wordmark_style).color(t.text_strong))
}

/// Row 2 search trigger button and right-aligned utility icon buttons
/// (`Filter by project`, `Add project`, `New thread`).
fn utility_bar(s: &Desktop, now: i64, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    filter::combobox(s, now, t, layer);

    let add_project = ui::icon_button(
        &ui::FOLDER_PLUS,
        "Add project",
        IconButtonVariant::GhostMuted,
        Size::Compact,
    )
    .disabled(!m.connected())
    .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Sidebar(SidebarMsg::OpenAddProject), tx))
    .build(&t.k)
    .key("add-project");

    row()
        .key("sidebar-action-bar")
        .w_full()
        .h(32.0)
        .mb(S6)
        .gap(S4)
        .center_y()
        .child(search_trigger(t))
        .child(
            row()
                .gap(S2)
                .center_y()
                .child(filter_trigger(s, t))
                .child(add_project)
                .child(new_thread_button(s, Size::Compact, t)),
        )
}

fn search_trigger(t: &Theme) -> El {
    ui::with_tooltip(
        row()
            .key("sidebar-search-trigger")
            .role("button")
            .label("Search")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .h(Size::Compact.height())
            .px(S8)
            .gap(S8)
            .center_y()
            .radius(RADIUS_CONTROL)
            .fill(Color::TRANSPARENT)
            .hover_group()
            .hover(t.hover_wash)
            .press(t.pressed_wash)
            .press_scale(motion::PRESS_SCALE)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Palette(PaletteMsg::Toggle), tx);
            })
            .child(
                icon(&ui::SEARCH, ICON_MD)
                    .color(t.icon_muted)
                    .group_color(t.text)
                    .a11y_hidden(),
            )
            .child(
                text("Search")
                    .font(t.panel_row)
                    .color(t.muted)
                    .group_color(t.text)
                    .truncate()
                    .a11y_hidden(),
            ),
        "Search",
        Some(&ui::shortcut("K")),
    )
}

fn filter_trigger(s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let filtered = m.sidebar.project_filter != ProjectFilter::All;
    let filter_active = filtered || m.sidebar.filter_open;
    let filter_tip = match &m.sidebar.project_filter {
        ProjectFilter::All => "Filter by project".to_owned(),
        ProjectFilter::NoProject => "Filtered: No project".to_owned(),
        ProjectFilter::Project(path) => {
            let name = m
                .cache()
                .project(path)
                .map_or(path.as_str(), |p| p.name.as_str());
            format!("Filtered: {name}")
        }
    };
    let mut btn = row()
        .key(filter::FILTER_TRIGGER_KEY)
        .role("button")
        .label(&filter_tip)
        .help(&filter_tip)
        .expanded(m.sidebar.filter_open)
        .size(Size::Compact.height(), Size::Compact.height())
        .center()
        .radius(Size::Compact.radius())
        .fill(if filter_active {
            t.accent.alpha(0.15)
        } else {
            Color::TRANSPARENT
        })
        .hover_group()
        .hover(if filter_active {
            t.accent.alpha(0.20)
        } else {
            t.hover_wash
        })
        .press(t.pressed_wash)
        .press_scale(motion::PRESS_SCALE)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Sidebar(SidebarMsg::ToggleProjectFilter), tx);
        })
        .child(
            icon(&ui::LIST_FILTER, 15.0)
                .color(if filter_active {
                    t.accent_text
                } else {
                    t.icon_muted
                })
                .group_color(if filter_active { t.accent_text } else { t.text })
                .a11y_hidden(),
        );
    if filter_active {
        btn = btn.border(HAIRLINE, t.accent.alpha(0.45));
    }
    btn
}

fn active_filter_chip(s: &Desktop, t: &Theme) -> Option<El> {
    let (mark, name) = filter::active_chip_parts(s, t)?;
    Some(
        row()
            .key("sidebar-active-filter")
            .w_full()
            .h(26.0)
            .mb(S6)
            .px(S8)
            .gap(S6)
            .center_y()
            .radius(RADIUS_CONTROL)
            .fill(t.surface_recessed)
            .border(HAIRLINE, t.hairline)
            .transition(motion::chip_enter())
            .child(mark)
            .child(
                text(name)
                    .font(t.small_medium)
                    .color(t.text_strong)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate(),
            )
            .child(
                ui::icon_button(
                    &ui::CLOSE,
                    "Clear filter",
                    IconButtonVariant::GhostMuted,
                    Size::Tiny,
                )
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::SetProjectFilter(ProjectFilter::All)),
                        tx,
                    );
                })
                .build(&t.k)
                .key("clear-project-filter"),
            ),
    )
}
