//! viz desktop app: conversations in a sidebar, the selected one's timeline,
//! and a composer, in viz's own theme (`theme.rs`) through rune-kit. `cargo
//! run -p viz-desktop` opens the window; `-- --snap out.png` renders it
//! headless.

mod app;
mod attachments;
mod backend;
mod changes;
mod composer;
mod config;
mod header;
mod palette;
mod scenes;
mod sidebar;
mod theme;
mod thread;
mod timeline;
mod ui;

use std::{path::PathBuf, time::Duration};

use clap::Parser;
use rune::{App, AppSpec, Chrome, Cx, El, KeyEvent, Tx, col};
use rune_kit::{
    ToastRegion,
    shell::{SidebarToggle, SplitView},
};
use tokio::sync::mpsc;
use viz_client::Endpoint;
use viz_config::{ClientFile, Paths};

use crate::{
    app::Desktop,
    config::DesktopConfig,
    theme::{S4, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT, Theme},
};

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
    // The sidebar's header row is a title band above its content: guard it
    // from below, under the sidebar toggle and new-chat button that float
    // over it.
    let sidebar = sidebar::sidebar(s, t, &mut overlays).child(
        ui::drag_guard("sidebar-drag-guard")
            .pin(
                Some(-SIDEBAR_PAD),
                Some(-TOOLBAR_HEIGHT),
                Some(-SIDEBAR_PAD),
                None,
            )
            .h(TOOLBAR_HEIGHT),
    );
    // The kit's shell, flat: no inset frame, the main pane square against
    // the sidebar, which sits on the deeper backdrop. The sidebar toggle ends
    // the sidebar's header row, centered on the toolbar's line and over the
    // rows' trailing slots.
    let split = SplitView::new(main).sidebar(&s.sidebar, |s: &mut Desktop| &mut s.sidebar, sidebar);
    // The orchestrator has no checkout: its chat never shows the changes
    // panel, which stays as it was for the next chat.
    let split = if s.orchestrator_open() {
        split
    } else {
        split.inspector(
            &s.changes.pane,
            |s: &mut Desktop| &mut s.changes.pane,
            changes::view(s, t, &mut overlays),
        )
    };
    let split = split
        .toolbar(header::toolbar(s, t, &mut overlays))
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(0.0)
        .toolbar_in_panel()
        .sidebar_padding(SIDEBAR_PAD, SIDEBAR_PAD)
        .sidebar_toggle(
            SidebarToggle::new()
                .size(Size::Default.kit())
                .gap(S4)
                .shortcut(ui::SIDEBAR_SHORTCUT)
                .in_header()
                .unread(
                    s.conversations
                        .iter()
                        .any(|chat| chat.unread && !chat.archived),
                ),
        )
        .reduced_motion(cx.is_reduced_motion())
        .build(&t.shell);
    let split = if s.sidebar.is_open() {
        let (x, y) = sidebar::new_chat_origin(s);
        split.child(sidebar::new_chat_button(s, t).absolute(x, y))
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
        region.toast(ui::toast(
            format!("toast-{id}"),
            &toast.title,
            toast.detail.as_deref(),
            ui::lead(ui::dot(t.danger), t.ui.line_height),
            move |s: &mut Desktop| s.dismiss(id),
            t,
        ))
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
    if ev.is_command('b') && !ev.mods.alt {
        if ev.mods.shift {
            header::toggle_agents(s);
        } else {
            s.sidebar.toggle();
        }
        return true;
    }
    if ev.mods.shift || ev.mods.alt {
        return false;
    }
    if ev.is_command('[') {
        s.go_back(tx);
        return true;
    }
    if ev.is_command(']') {
        s.go_forward(tx);
        return true;
    }
    if ev.is_command('n') {
        s.create_here(tx);
        return true;
    }
    if ev.is_command('d') && !s.orchestrator_open() {
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

#[derive(Parser)]
#[command(disable_help_flag = true)]
struct Args {
    /// Use this desktop config file instead of the environment or default
    /// location.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Strip viz-specific flags before Rune handles its own tooling arguments.
fn args() -> (ClientFile, Vec<String>) {
    let mut input = std::env::args();
    let mut viz: Vec<_> = input.next().into_iter().collect();
    let mut rune = Vec::new();
    while let Some(arg) = input.next() {
        if arg == "--config" {
            viz.push(arg);
            viz.extend(input.next());
        } else if arg.starts_with("--config=") {
            viz.push(arg);
        } else {
            rune.push(arg);
        }
    }
    let args = Args::parse_from(viz);
    (
        args.config
            .map_or_else(ClientFile::desktop, ClientFile::path),
        rune,
    )
}

fn main() {
    let (file, rune_args) = args();
    let tooling = rune::tooling::Args::parse_from(rune_args.clone());
    let headless = tooling.headless();
    let preview = headless && tooling.scene.is_some();
    let scene_theme = preview
        .then(|| std::env::var("VIZ_SCENE_THEME").ok())
        .flatten();
    let headless_dark = headless.then_some(tooling.theme.as_deref() != Some("light"));
    let (theme_error, endpoint) = match viz_config::load::<DesktopConfig>(&file) {
        Ok(config) => {
            let endpoint = Paths::from_env()
                .map_err(|e| e.to_string())
                .and_then(|paths| {
                    Endpoint::from_connection(&config.backend, &paths).map_err(|e| e.to_string())
                });
            (
                theme::install(&config.theme, scene_theme.as_deref()),
                endpoint,
            )
        }
        Err(error) => {
            let _ = theme::install(&DesktopConfig::default().theme, scene_theme.as_deref());
            let detail = error.to_string();
            (Some(detail.clone()), Err(detail))
        }
    };
    rune::text::set_icon_swap(Some(ui::swap_kit_icon));
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
    .args(rune_args)
    // macOS: the traffic lights centered on the toolbar's line, at the
    // sidebar's content inset.
    .chrome(Chrome {
        traffic_lights: Some((
            theme::SIDEBAR_INSET,
            (theme::TOOLBAR_HEIGHT - Chrome::TRAFFIC_LIGHT) / 2.0,
        )),
        ..Chrome::DEFAULT
    })
    .init(move || {
        let mut desktop = Desktop::new(commands);
        // Scenes are deterministic: they neither read nor write the saved
        // sidebar mode and collapsed projects.
        if !preview {
            desktop.navigation.restore();
        }
        // Offscreen snapshots can draw only once, before the frame callback.
        if let Some(dark) = headless_dark {
            desktop.changes.set_dark(dark);
        }
        desktop
    })
    .on_focus(Desktop::set_focus);
    if let Some(detail) = theme_error {
        app.handle()
            .update(move |s, tx| s.toast(tx, "Invalid desktop configuration", Some(detail)));
    }
    // Rune has no resize or appearance hook; a drawn frame reports the
    // viewport and appearance, and a change re-lays out what depends on it
    // (the composer's selects) or recolors it (the diff's code).
    let ui = app.handle();
    let app = app.on_frame_drawn(move |rt| {
        let width = rt.window_viewport().0 / rt.zoom();
        if (rt.state.window_width - width).abs() >= 1.0 {
            ui.update(move |s, _| s.window_width = width);
        }
        let dark = rt.env.system_dark;
        if rt.state.changes.dark() != Some(dark) {
            ui.update(move |s, _| s.changes.set_dark(dark));
        }
    });
    if !preview {
        let (first, connected) = std::sync::mpsc::channel();
        backend::spawn(app.handle(), receiver, first, endpoint);
        if headless {
            // The default snapshot shows the first connection attempt's
            // outcome.
            _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
        }
    }
    app.run();
}
