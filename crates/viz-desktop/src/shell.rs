//! Window shell layout with explicit slots for `sidebar`, `toolbar`, `main`,
//! `diff`, and `overlays`.

use std::time::Duration;

use rune::{El, col, el};
use rune_kit::{
    Toast, ToastRegion,
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
    /// Optional resizable right-hand diff inspector slot.
    pub diff: Option<El>,
    /// Popovers, menus, dialogs, palette, and sheets rendered above the shell.
    pub overlays: Vec<El>,
}

/// Composes `slots` into the root window element with pane motion, titlebar
/// insets, drag guards, resize accent hairline, and floating toasts.
pub fn build(s: &Desktop, slots: ShellSlots, titlebar_inset: f32, reduced: bool, t: &Theme) -> El {
    let ShellSlots {
        sidebar,
        toolbar,
        main,
        diff,
        mut overlays,
    } = slots;
    let main = motion::stable_content(
        col()
            .w_full()
            .h_full()
            .min_h(0.0)
            .mt(crate::header::CHAT_HEADER_HEIGHT - TOOLBAR_HEIGHT)
            .children((!crate::home::is_active(&s.model)).then(|| ui::rule(t).key("header-rule")))
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
    rune_kit::root(split, overlays)
        .hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
        .on_layout_tx(|s: &mut Desktop, [_, _, width, height], tx| {
            let panels = &s.model.panels;
            if (panels.window_width - width).abs() >= 1.0
                || (panels.window_height - height).abs() >= 1.0
            {
                s.dispatch(
                    crate::update::Msg::Shell(crate::update::ShellMsg::WindowResized(
                        width, height,
                    )),
                    tx,
                );
            }
        })
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
            let mut card = Toast::new(format!("toast-{id}"), &entry.options.title)
                .lead(entry.options.tone.mark(t))
                .on_close(move |s: &mut Desktop| s.dismiss_toast_local(id));
            if let Some(detail) = &entry.options.detail {
                card = card.description(detail);
            }
            if let Some(undo) = entry.options.undo.clone() {
                card = card.action(&t.overlay, "Undo", true, move |s: &mut Desktop, tx| {
                    s.dismiss_toast(id, tx);
                    s.dispatch(Msg::Sidebar(SidebarMsg::UndoTriage(undo.clone())), tx);
                });
            }
            region.toast(card.build(&t.overlay))
        });
    let region = region.build(&t.k);
    let region = if entries[start..]
        .iter()
        .any(|entry| entry.options.undo.is_some())
    {
        region.pin(None, None, Some(32.0), Some(128.0))
    } else {
        region
    };
    Some(region.on_hover_change(|s: &mut Desktop, inside, tx| s.toast_hover(inside, tx)))
}
