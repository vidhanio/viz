//! viz desktop app: conversations in a sidebar, the selected one's timeline,
//! and a composer, in viz's own theme (`theme.rs`) through rune-kit. `cargo
//! run -p viz-desktop` opens the window; `-- --snap out.png` renders it
//! headless.

mod app;
mod attachments;
mod backend;
mod changes;
mod composer;
mod header;
mod palette;
mod scenes;
mod sidebar;
mod theme;
mod thread;
mod timeline;
mod ui;

use std::time::Duration;

use rune::{App, AppSpec, Cx, El, KeyEvent, Tx, col, row};
use rune_kit::{
    Toast, ToastRegion,
    shell::{self, SidebarToggle, SplitView},
};
use tokio::sync::mpsc;

use crate::{app::Desktop, theme::Theme};

/// Longest a headless run waits for the first connection attempt before
/// rendering.
const HEADLESS_CONNECT_WAIT: Duration = Duration::from_secs(5);

fn view(s: &Desktop, cx: &mut Cx<Desktop>) -> El {
    let t = Theme::get(cx.system_dark());
    let mut overlays = Vec::new();
    let main = col()
        .key("main")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(ui::rule(t).key("header-rule"))
        .child(timeline::timeline(s, t));
    let main = main.children(composer::composer(s, t, &mut overlays));
    // The kit's shell, flat: no inset frame, the main pane square against
    // the sidebar, which sits on the deeper backdrop.
    let split = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut Desktop| &mut s.sidebar,
            sidebar::sidebar(s, t, &mut overlays),
        )
        .inspector(
            &s.changes.pane,
            |s: &mut Desktop| &mut s.changes.pane,
            changes::view(s, t, &mut overlays),
        )
        .toolbar(header::toolbar(s, t))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(0.0)
        .toolbar_in_panel()
        .sidebar_toggle(
            SidebarToggle::new().in_header().unread(
                s.conversations
                    .iter()
                    .any(|chat| chat.unread && !chat.archived),
            ),
        )
        .reduced_motion(cx.is_reduced_motion())
        .build(&t.shell);
    let split = if s.sidebar.is_open() {
        // In the shell's header band, left of the sidebar toggle (macOS: past
        // the traffic lights).
        let left = cx.titlebar_inset() + 8.0;
        split.child(
            row()
                .absolute(left, 12.0)
                .child(sidebar::new_chat_button(s, t)),
        )
    } else {
        split
    };
    sidebar::overlays(s, t, &mut overlays);
    changes::overlays(s, t, &mut overlays);
    palette::mount(s, t, &mut overlays);
    overlays.extend(toasts(s, t));
    rune_kit::root(split, overlays)
}

/// The toast stack, when there's anything in it.
fn toasts(s: &Desktop, t: &Theme) -> Option<El> {
    if s.toasts.is_empty() {
        return None;
    }
    let region = s.toasts.iter().fold(ToastRegion::new(), |region, toast| {
        let id = toast.id;
        let mut built = Toast::new(format!("toast-{id}"), toast.title.clone())
            .lead(ui::dot(t.danger, 7.0))
            .on_close(move |s: &mut Desktop| s.dismiss(id));
        if let Some(detail) = &toast.detail {
            built = built.description(detail.clone());
        }
        region.toast(built.build(&t.k))
    });
    Some(region.build(&t.k))
}

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('k') && !ev.mods.shift && !ev.mods.alt {
        palette::toggle(s, tx);
        return true;
    }
    if s.palette.is_open() {
        return false;
    }
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.mods.shift || ev.mods.alt {
        return false;
    }
    if ev.is_command('n') {
        s.create_here(tx);
        return true;
    }
    if ev.is_command('d') {
        changes::toggle(s, tx);
        return true;
    }
    if ev.is_command('.') && s.busy() {
        s.abort(tx);
        return true;
    }
    false
}

fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let completion = composer::poll(s, clock, tx);
    let palette = palette::poll(s, clock, tx);
    s.expire(clock) || completion || palette
}

fn main() {
    let (commands, receiver) = mpsc::unbounded_channel();
    let app = App::new(AppSpec {
        min_size: Some((720.0, 480.0)),
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        scenes: scenes::SCENES,
        on_key: Some(on_key),
        poll: Some(poll),
        ..AppSpec::new("viz", app::WINDOW_SIZE, Desktop::detached, view)
    })
    .init(move || Desktop::new(commands))
    .on_focus(Desktop::set_focus);
    // Rune has no resize hook; a drawn frame reports the viewport, and a change
    // re-lays out what depends on it (the composer's selects).
    let ui = app.handle();
    let app = app.on_frame_drawn(move |rt| {
        let width = rt.window_viewport().0 / rt.zoom();
        if (rt.state.window_width - width).abs() >= 1.0 {
            ui.update(move |s, _| s.window_width = width);
        }
    });
    let headless = rune::is_headless();
    let preview = headless && std::env::args_os().any(|arg| arg == "--scene");
    if !preview {
        let (first, connected) = std::sync::mpsc::channel();
        backend::spawn(app.handle(), receiver, first);
        if headless {
            // The default snapshot shows the first connection attempt's
            // outcome.
            _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
        }
    }
    app.run();
}
