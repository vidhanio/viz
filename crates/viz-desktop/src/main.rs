//! viz desktop app: threads in a sidebar, the selected one's timeline,
//! and a composer, in viz's own theme (`theme.rs`) through rune-kit. `cargo
//! run -p viz-desktop` opens the window; `-- --snap out.png --scene <name>`
//! renders a scene headless.
//!
//! UI rules:
//!
//! - Layout: sidebar and thread. A thread with delegated tasks keeps their
//!   panel at the top right. Menus, palette, dialogs and toasts are temporary
//!   overlays.
//! - Sidebar: pinned threads, _Recent_ (newest first, project under each title)
//!   and _Projects_ (collapsible) modes, and an environment picker in the
//!   footer when more than one is configured. Task status belongs in the task
//!   panel and timeline.
//! - Activity: reasoning, tools and delegated tasks fold into compact groups;
//!   running commands stay visible while folded; expand only for readable
//!   output, changes or the reasoning trace. Context use sits beside the
//!   composer options.
//! - Copy: short, plain, sentence case; labels name the thing, buttons name the
//!   action. Show display names, never wire IDs.
//! - Visuals come from `theme.rs` tokens and `ui` components. Shared scales and
//!   columns are tokens; a component-specific cap may be a named module
//!   constant on the 4-pt grid. Icons are Lucide (harnesses may use their
//!   official marks); never draw custom ones. Surfaces are flat, separated by
//!   hairlines and fills; overlays also take a hairline outline and shadow. One
//!   accent, for the primary action, focus and selection.
//! - Motion: changing threads moves the transcript and toolbar title, never the
//!   composer: vertically by sidebar order, horizontally into a delegated task
//!   (left) and back out (right); short springs without bounce.
//! - One obvious place per action, plus the palette and a shortcut where it is
//!   frequent.

mod app;
mod attachments;
mod composer;
mod config;
mod header;
mod palette;
mod scenes;
mod server;
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
use viz_config::ClientFile;

use crate::{
    app::Desktop,
    config::DesktopConfig,
    theme::{ICON, S4, SIDEBAR_INSET, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT, Theme},
};

/// Longest a headless run waits for the first connection attempt before
/// rendering.
const HEADLESS_CONNECT_WAIT: Duration = Duration::from_secs(5);

fn view(s: &Desktop, cx: &mut Cx<Desktop>) -> El {
    let t = Theme::get(cx.system_dark());
    let mut overlays = Vec::new();
    let (agents, reserve) = header::agents(s, t);
    let reading = col()
        .key("reading-pane")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .pr(reserve)
        .child(timeline::timeline(s, t, &mut overlays))
        .children(composer::composer(s, t, &mut overlays));
    let main = col()
        .key("main")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(ui::rule(t).key("header-rule"))
        .children(agents)
        .child(reading);
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
    let titlebar_inset = cx.titlebar_inset();
    let split = SplitView::new(main).sidebar(&s.sidebar, |s: &mut Desktop| &mut s.sidebar, sidebar);
    let split = split
        .toolbar(header::toolbar(s, t))
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
                .unread(
                    s.threads
                        .iter()
                        .any(|thread| thread.unread && !thread.archived),
                ),
        )
        .reduced_motion(cx.is_reduced_motion())
        .build(&t.shell);
    let split = if s.sidebar.is_open() {
        let left = if titlebar_inset > 0.0 {
            titlebar_inset + SIDEBAR_PAD
        } else {
            SIDEBAR_INSET - (Size::Default.height() - ICON) / 2.0
        };
        let top = (TOOLBAR_HEIGHT - Size::Default.height()) / 2.0;
        split.child(sidebar::header_buttons(s, t).absolute(left, top))
    } else {
        split
    };
    sidebar::overlays(s, t, &mut overlays);
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
    if ev.is_command('b') && !ev.mods.shift && !ev.mods.alt {
        s.sidebar.toggle();
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
    if ev.is_command('.') && s.busy() {
        s.abort(tx);
        return true;
    }
    false
}

fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let completion = composer::poll(s, clock, tx);
    let palette = palette::poll(s, clock, tx);
    let tool_timer = if s.thread.acp_partial.iter().any(|item| {
        matches!(item, thread::Item::Tool(tool) if tool.status == thread::ToolStatus::Running
            && tool.started_at.is_some() && tool.timeout_ms.is_some())
    }) {
        let now = theme::now_ms();
        tx.wake_after(Duration::from_millis(500));
        if now.saturating_sub(s.tool_clock) >= 500 {
            s.tool_clock = now;
            true
        } else {
            false
        }
    } else {
        false
    };
    s.expire(clock) || completion || palette || tool_timer
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "viz_desktop=warn,viz_client=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let (file, rune_args) = args();
    let tooling = rune::tooling::Args::parse_from(rune_args.clone());
    let headless = tooling.headless();
    let preview = headless && tooling.scene.is_some();
    let initial_width = tooling.size.map_or(app::WINDOW_SIZE.0, |(width, _)| width);
    let scene_theme = preview
        .then(|| std::env::var("VIZ_SCENE_THEME").ok())
        .flatten();
    let (theme_error, endpoint) = match viz_config::load::<DesktopConfig>(&file) {
        Ok(config) => {
            let endpoint = Endpoint::from_connection(&config.server).map_err(|e| e.to_string());
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
        fonts: &[],
        families: theme::families(),
        scenes: scenes::SCENES,
        on_key: Some(on_key),
        poll: Some(poll),
        ..AppSpec::new("viz", app::WINDOW_SIZE, Desktop::detached, view)
    })
    .args(rune_args)
    .chrome(Chrome {
        full_size_content: cfg!(target_os = "macos"),
        traffic_lights: cfg!(target_os = "macos").then_some((
            theme::SIDEBAR_INSET,
            (theme::TOOLBAR_HEIGHT - Chrome::TRAFFIC_LIGHT) / 2.0,
        )),
        ..Chrome::DEFAULT
    })
    .init(move || {
        let mut desktop = Desktop::new(commands);
        desktop.window_width = initial_width;
        if !preview {
            desktop.navigation.restore();
        }
        desktop
    })
    .on_focus(Desktop::set_focus);
    if let Some(detail) = theme_error {
        app.handle()
            .update(move |s, tx| s.toast(tx, "Invalid desktop configuration", Some(detail)));
    }
    let ui = app.handle();
    let app = app.on_frame_drawn(move |rt| {
        let width = rt.window_viewport().0 / rt.zoom();
        if (rt.state.window_width - width).abs() >= 1.0 {
            ui.update(move |s, _| s.window_width = width);
        }
    });
    if !preview {
        let (first, connected) = std::sync::mpsc::channel();
        server::spawn(app.handle(), receiver, first, endpoint);
        if headless {
            _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
        }
    }
    app.run();
}
