//! viz desktop app: conversations in a sidebar, the selected one's timeline,
//! and a composer, in viz's own theme (`theme.rs`) through rune-kit. `cargo
//! run -p viz-desktop` opens the window; `-- --snap out.png` renders it
//! headless.

mod app;
mod backend;
mod composer;
mod header;
mod sidebar;
mod theme;
mod thread;
mod timeline;

use std::time::Duration;

use rune::{App, AppSpec, Cx, El, KeyEvent, Tx, col, el};
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
    let main = col()
        .key("main")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(el().key("header-rule").w_full().h(1.0).fill(t.hairline))
        .child(timeline::timeline(s, t));
    let main = if s.can_compose() {
        main.child(composer::composer(s, t))
    } else {
        main
    };
    // The kit's shell, flat: no inset frame, the main pane square against
    // the sidebar, which sits on the deeper backdrop.
    let split = SplitView::new(main)
        .sidebar(
            &s.sidebar,
            |s: &mut Desktop| &mut s.sidebar,
            sidebar::sidebar(s, t),
        )
        .toolbar(header::toolbar(s, t))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(0.0)
        .toolbar_in_panel()
        .sidebar_toggle(SidebarToggle::new().in_header())
        .reduced_motion(cx.is_reduced_motion())
        .build(&t.shell);
    rune_kit::root(split, toasts(s, t))
}

/// The toast stack, when there's anything in it.
fn toasts(s: &Desktop, t: &Theme) -> Option<El> {
    if s.toasts.is_empty() {
        return None;
    }
    let region = s.toasts.iter().fold(ToastRegion::new(), |region, toast| {
        let id = toast.id;
        let mut built = Toast::new(format!("toast-{id}"), toast.title.clone())
            .lead(theme::dot(t.danger, 7.0))
            .on_close(move |s: &mut Desktop| s.dismiss(id));
        if let Some(detail) = &toast.detail {
            built = built.description(detail.clone());
        }
        region.toast(built.build(&t.k))
    });
    Some(region.build(&t.k))
}

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.is_command('n') && !ev.mods.shift {
        s.create(tx);
        return true;
    }
    false
}

fn poll(s: &mut Desktop, clock: f64, _: &mut Tx) -> bool {
    s.expire(clock)
}

fn main() {
    let (commands, receiver) = mpsc::unbounded_channel();
    let app = App::new(AppSpec {
        min_size: Some((720.0, 480.0)),
        fonts: theme::FONTS,
        families: theme::FAMILIES,
        on_key: Some(on_key),
        poll: Some(poll),
        ..AppSpec::new("viz", (1120.0, 740.0), Desktop::detached, view)
    })
    .init(move || Desktop::new(commands));
    let (first, connected) = std::sync::mpsc::channel();
    backend::spawn(app.handle(), receiver, first);
    if rune::is_headless() {
        // A snapshot shows the first attempt's outcome, not a race with it.
        _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
    }
    app.run();
}
