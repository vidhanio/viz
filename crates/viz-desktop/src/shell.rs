//! Window shell layout with explicit slots for `sidebar`, `toolbar`, `main`,
//! `details`, `diff`, `terminal`, and `overlays`.
//!
//! Slots not yet populated by Phase 1 feature tracks (`details`, `diff`,
//! `terminal`) stay `None` and add zero layout nodes or visual chrome.

use rune::{El, col, row};
use rune_kit::{
    ToastRegion,
    shell::{SidebarToggle, SplitView, Toolbar},
};

use crate::{
    app::Desktop,
    motion,
    theme::{S4, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT, Theme},
    ui,
};

/// Explicit structural slots composed by the desktop window shell.
pub struct ShellSlots {
    /// Leading workspace sidebar content.
    pub sidebar: El,
    /// Top toolbar inside the main panel.
    pub toolbar: Toolbar,
    /// Primary thread or home content (timeline + composer).
    pub main: El,
    /// Optional right-hand thread details card slot (Phase 1 Track G).
    pub details: Option<El>,
    /// Optional resizable right-hand diff inspector slot (Phase 1 Track G).
    pub diff: Option<El>,
    /// Optional bottom docked terminal drawer slot (Phase 1 Track X).
    pub terminal: Option<El>,
    /// Popovers, menus, dialogs, palette, and sheets rendered above the shell.
    pub overlays: Vec<El>,
}

/// Composes `slots` into the root window element with pane motion, titlebar
/// insets, drag guards, and floating toasts.
pub fn build(s: &Desktop, slots: ShellSlots, titlebar_inset: f32, reduced: bool, t: &Theme) -> El {
    let ShellSlots {
        sidebar,
        toolbar,
        mut main,
        details,
        diff,
        terminal,
        mut overlays,
    } = slots;
    if let Some(details) = details {
        main = row()
            .key("main-with-details")
            .w_full()
            .h_full()
            .min_h(0.0)
            .items_start()
            .child(main.grow(1.0).shrink().min_w(0.0))
            .child(details);
    }
    if let Some(terminal) = terminal.filter(|_| s.model.panels.terminal_open) {
        main = col()
            .key("main-with-terminal")
            .w_full()
            .h_full()
            .min_h(0.0)
            .child(main.grow(1.0).shrink().min_h(0.0))
            .child(terminal.h(crate::model::terminal::height(&s.model)));
    }
    let main = motion::stable_content(
        col()
            .w_full()
            .h_full()
            .min_h(0.0)
            .mt(crate::header::CHAT_HEADER_HEIGHT - TOOLBAR_HEIGHT)
            .child(ui::rule(t).key("header-rule"))
            .child(main.grow(1.0)),
    );
    let sidebar = sidebar.child(
        ui::drag_guard("sidebar-drag-guard")
            .pin(
                Some(-SIDEBAR_PAD),
                Some(-TOOLBAR_HEIGHT),
                Some(-SIDEBAR_PAD),
                None,
            )
            .h(TOOLBAR_HEIGHT),
    );
    let mut split = SplitView::new(main).sidebar(
        &s.widgets.sidebar_pane,
        |s: &mut Desktop| &mut s.widgets.sidebar_pane,
        sidebar,
    );
    if let Some(diff) = diff {
        split = split.inspector(
            &s.widgets.diff_pane,
            |s: &mut Desktop| &mut s.widgets.diff_pane,
            diff,
        );
    }
    let split = split
        .toolbar(toolbar)
        .titlebar_inset(titlebar_inset)
        .frame_inset(0.0)
        .toolbar_in_panel()
        .sidebar_padding(SIDEBAR_PAD, SIDEBAR_PAD)
        .sidebar_toggle(
            SidebarToggle::new()
                .size(Size::Default.kit())
                .gap(S4)
                .shortcut(ui::SIDEBAR_SHORTCUT)
                .in_header()
                .unread(s.model.has_unread()),
        )
        .pane_motion(motion::pane_motion())
        .reduced_motion(reduced)
        .build(&t.shell);
    overlays.extend(toasts(s, t));
    rune_kit::root(split, overlays).hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
}

/// The floating toast stack, when non-empty.
fn toasts(s: &Desktop, t: &Theme) -> Option<El> {
    let entries = s.widgets.toasts.entries();
    if entries.is_empty() {
        return None;
    }
    let start = entries.len().saturating_sub(rune_kit::toast_stack::VISIBLE);
    let region = entries[start..]
        .iter()
        .fold(ToastRegion::new(), |region, entry| {
            let id = entry.id;
            region.toast(ui::toast(
                format!("toast-{id}"),
                &entry.options.title,
                entry.options.detail.as_deref(),
                ui::lead(ui::dot(t.danger), t.ui.line_height),
                move |s: &mut Desktop, tx| s.dismiss_toast(id, tx),
                t,
            ))
        });
    Some(
        region
            .build(&t.k)
            .on_hover_change(|s: &mut Desktop, inside, tx| s.toast_hover(inside, tx)),
    )
}
