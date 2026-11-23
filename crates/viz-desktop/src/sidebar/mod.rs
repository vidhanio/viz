//! Sidebar view: filter, pinned threads, Recent / Projects modes, delegated
//! task subtrees, context menus, confirmation dialogs, and connection/env
//! footer.

mod actions;
mod footer;
mod rows;

use rune::{Color, El, col, el, row, text};
use rune_kit::{IconButtonVariant, shell::Sidebar};

use crate::{
    app::Desktop,
    model::{SidebarMode, SidebarOutline},
    theme::{self, HAIRLINE, ROW_INSET, S8, S12, Size, Theme},
    ui,
    update::{Msg, SelectionMsg, SidebarMsg},
};

/// Builds the workspace sidebar view and mounts its popovers into `layer`.
pub fn sidebar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let tree = thread_tree(s, t, layer);
    let mut bar = Sidebar::new()
        .label("Workspace")
        .inset(0.0)
        .header_gap(S8)
        .header_padding(0.0, 0.0)
        .header(filter(s, t))
        .child(tree);
    if let Some(footer) = footer::footer(s, t, layer) {
        bar = bar.footer(footer);
    }
    bar.build(&t.k)
}

/// Mounts sidebar confirmation and rename dialogs independently of whether
/// the sidebar is visible.
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    actions::dialog(s, t, layer);
}

/// New thread, as an icon button: in the sidebar's mode row while the sidebar
/// is open, after the sidebar toggle in the toolbar while it's closed.
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
    .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Selection(SelectionMsg::CreateHere), tx))
    .build(&t.k)
    .key("new-thread")
}

fn add_project_button(s: &Desktop, t: &Theme) -> El {
    ui::icon_button(
        &ui::FOLDER_PLUS,
        "Add project",
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .disabled(!s.model.connected())
    .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Sidebar(SidebarMsg::OpenAddProject), tx))
    .build(&t.k)
    .key("add-project")
}

fn filter(s: &Desktop, t: &Theme) -> El {
    ui::text_field(
        &s.widgets.sidebar_filter,
        "Filter threads",
        "Filter threads",
        Size::Default,
    )
    .leading_icon(&ui::SEARCH)
    .on_edit(|s: &mut Desktop| &mut s.widgets.sidebar_filter)
    .on_change(|s: &mut Desktop, tx| {
        let query = s.widgets.sidebar_filter.text.clone();
        s.dispatch(Msg::Sidebar(SidebarMsg::FilterChanged(query)), tx);
    })
    .on_submit(|s: &mut Desktop, tx| s.dispatch(Msg::Sidebar(SidebarMsg::FilterSubmit), tx))
    .build(&t.k)
    .key("thread-filter")
}

fn thread_tree(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let outline = SidebarOutline::derive(m, theme::now_ms());
    let recent = m.sidebar.mode == SidebarMode::Recent;
    let mut tree = col()
        .key("thread-tree")
        .role("tree")
        .label("Threads")
        .w_full();
    if !outline.pinned.is_empty() {
        tree =
            tree.child(ui::section_row("pinned-heading", "Pinned", None, t))
                .children(outline.pinned.iter().map(|thread| {
                    rows::thread_row(s, &outline, thread, t.muted, recent, t, layer)
                }));
    }
    tree = tree.child(mode_switch(s, t));
    if recent {
        if outline.pinned.is_empty() && outline.recent.is_empty() {
            return tree.children(empty_note(s, &outline.query, t));
        }
        return tree.children(
            outline
                .recent
                .iter()
                .map(|thread| rows::thread_row(s, &outline, thread, t.muted, true, t, layer)),
        );
    }
    let projects: Vec<_> = m
        .cache()
        .projects()
        .filter(|p| outline.visible_project(p))
        .collect();
    if !projects.is_empty() {
        tree = tree.child(
            col().key("project-groups").w_full().gap(S12).children(
                projects
                    .iter()
                    .map(|p| rows::project_row(s, &outline, p, t, layer)),
            ),
        );
    }
    if !outline.loose.is_empty() {
        tree =
            tree.child(ui::section_row("threads-heading", "No project", None, t))
                .children(
                    outline.loose.iter().map(|thread| {
                        rows::thread_row(s, &outline, thread, t.muted, false, t, layer)
                    }),
                );
    }
    if outline.pinned.is_empty() && projects.is_empty() && outline.loose.is_empty() {
        tree = tree.children(empty_note(s, &outline.query, t));
    }
    tree
}

fn empty_note(s: &Desktop, query: &str, t: &Theme) -> Option<El> {
    let label = if !query.is_empty() {
        "No matching threads"
    } else if s.model.connected() {
        "No threads yet"
    } else {
        return None;
    };
    Some(
        text(label)
            .key("sidebar-empty")
            .role("status")
            .font(t.small)
            .color(t.faint)
            .px(ROW_INSET)
            .py(S8),
    )
}

/// The section row that picks how threads are listed, with the new thread and
/// add project actions at its end.
fn mode_switch(s: &Desktop, t: &Theme) -> El {
    let current = s.model.sidebar.mode;
    let tab = |mode: SidebarMode, label: &'static str| {
        let active = mode == current;
        row()
            .key(mode.as_str())
            .role("radio")
            .label(label)
            .checked(active)
            .h(Size::Small.height())
            .px(S8)
            .center()
            .radius(Size::Small.radius())
            .fill(if active {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press(move |s: &mut Desktop| {
                s.dispatch_local(Msg::Sidebar(SidebarMsg::SetMode(mode)));
            })
            .child(
                ui::section(label, t)
                    .color(if active { t.text } else { t.muted })
                    .group_color(t.text)
                    .a11y_hidden(),
            )
    };
    row()
        .key("sidebar-mode")
        .role("radiogroup")
        .label("List threads by")
        .w_full()
        .h(theme::TREE_ROW)
        .mt(theme::S12)
        .mb(theme::S4)
        .gap(theme::S4)
        .center_y()
        .child(tab(SidebarMode::Recent, "Recent"))
        .child(tab(SidebarMode::Projects, "Projects"))
        .child(el().grow(1.0).a11y_hidden())
        .child(new_thread_button(s, Size::Small, t))
        .child(add_project_button(s, t))
}
