//! Window shell layout with explicit slots for `sidebar`, `toolbar`, `main`,
//! `details`, `diff`, `terminal`, and `overlays`.
//!
//! Slots not yet populated by Phase 1 feature tracks (`details`, `diff`,
//! `terminal`) stay `None` and add zero layout nodes or visual chrome.

use std::time::Duration;

use rune::{El, col, el, row};
use rune_kit::{
    ButtonVariant, ToastRegion,
    shell::{SidebarPeek, SidebarToggle, SplitView, Toolbar},
};

use crate::{
    app::Desktop,
    motion,
    theme::{HAIRLINE, S8, S12, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT, Theme},
    ui,
    update::{Msg, SidebarMsg},
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
/// insets, drag guards, resize accent hairline, and floating toasts.
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
    let mut sidebar = sidebar.child(
        ui::drag_guard("sidebar-drag-guard")
            .pin(
                Some(-SIDEBAR_PAD),
                Some(-TOOLBAR_HEIGHT),
                Some(-SIDEBAR_PAD),
                None,
            )
            .h(TOOLBAR_HEIGHT),
    );
    if s.widgets.sidebar_pane.is_resizing() {
        sidebar = sidebar.child(
            el().key("sidebar-resize-accent")
                .pin(None, Some(-TOOLBAR_HEIGHT), Some(-SIDEBAR_PAD), Some(0.0))
                .w(HAIRLINE)
                .fill(t.accent)
                .a11y_hidden(),
        );
    }
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
                .size(Size::Compact.kit())
                .edge(S12)
                .gap(S8)
                .shortcut(ui::SIDEBAR_SHORTCUT)
                .unread(s.model.has_unread()),
        )
        .sidebar_peek(
            SidebarPeek::new()
                .edge(6.0, Duration::from_millis(40))
                .toggle_delay(Duration::from_millis(80))
                .reduced_motion_fade(0.12),
        )
        .pane_motion(motion::pane_motion())
        .reduced_motion(reduced)
        .build(&t.shell);
    overlays.extend(toasts(s, t));
    rune_kit::root(split, overlays).hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
}

/// The floating toast stack, including triage undo toasts, when non-empty.
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
            let mut card = ui::toast_card(format!("toast-{id}"), &entry.options.title)
                .tone(entry.options.tone)
                .on_close(move |s: &mut Desktop, tx| s.dismiss_toast(id, tx), t);
            if let Some(detail) = &entry.options.detail {
                card = card.detail(detail);
            }
            if let Some(undo) = entry.options.undo.clone() {
                let undo_btn = ui::button("Undo", ButtonVariant::Neutral, Size::Small)
                    .on_press_tx(move |s: &mut Desktop, tx| {
                        s.dismiss_toast(id, tx);
                        s.dispatch(Msg::Sidebar(SidebarMsg::UndoTriage(undo.clone())), tx);
                    })
                    .build(&t.overlay)
                    .key("undo");
                card = card.action(undo_btn);
            }
            region.toast(card.build(t))
        });
    Some(
        region
            .build(&t.k)
            .on_hover_change(|s: &mut Desktop, inside, tx| s.toast_hover(inside, tx)),
    )
}
