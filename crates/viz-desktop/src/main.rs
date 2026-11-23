//! viz desktop app: threads in a sidebar, the selected one's timeline,
//! and a composer, in viz's own theme (`theme`) through `rune-kit`. `cargo
//! run -p viz-desktop` opens the window; `-- --snap out.png --scene <name>`
//! renders a scene headless.
//!
//! Architecture: functional core (`model/`, `update/`) with edge effects
//! (`effects/`) and pure views emitting [`update::Msg`].
//!
//! UI rules:
//!
//! - Layout: shell slots for sidebar, main (timeline + composer + Tasks Dock),
//!   details, diff, terminal, and overlays (menus, palette, dialogs, toasts).
//! - Visuals come from `theme` tokens and `ui` components, derived strictly
//!   from the configured base16 scheme in a flat, sans-serif style with crisp
//!   hairline borders and no blur, glow, or drop shadows. Icons are Lucide plus
//!   monochrome harness marks.
//! - Motion: tokens and reusable primitives live in `motion`. Every spring is
//!   critically damped (zero bounce) and preserves velocity on mid-flight
//!   interruption; Reduce Motion removes spatial travel and stops live loops.

mod app;
mod attachments;
mod checkout;
mod composer;
mod config;
mod effects;
mod header;
mod model;
mod motion;
mod motion_scene;
mod palette;
mod scenes;
mod sidebar;
mod terminal;
mod theme;
mod timeline;
mod ui;
mod update;

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
    model::{PaletteSearch, WINDOW_SIZE},
    theme::{S4, SIDEBAR_PAD, Size, TOOLBAR_HEIGHT, Theme},
    update::{ComposerMsg, Msg, PaletteMsg, SelectionMsg, ShellMsg, SidebarMsg, TimelineMsg},
};

/// Longest a headless run waits for the first connection attempt before
/// rendering.
const HEADLESS_CONNECT_WAIT: Duration = Duration::from_secs(5);

fn view(s: &Desktop, cx: &mut Cx<Desktop>) -> El {
    if let Some(reduced) = s.reduced_motion {
        cx.reduced_motion(reduced);
    }
    let reduced = cx.is_reduced_motion();
    let t = Theme::get(cx.system_dark());
    cx.help_look(t.help_look());
    if let Some(terminal) = terminal::scene::view(t) {
        return terminal;
    }
    if let Some(demo) = &s.motion_demo {
        return motion_scene::view(demo, reduced, t);
    }
    let mut overlays = Vec::new();
    let dock = composer::composer(s, t, &mut overlays);
    let mut main = col()
        .key("main")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(ui::rule(t).key("header-rule"))
        .child(timeline::timeline(s, t, &mut overlays));
    if let Some(dock) = dock {
        main = main.child(dock);
    }
    let main = motion::stable_content(main);
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
    let split = SplitView::new(main)
        .sidebar(
            &s.widgets.sidebar_pane,
            |s: &mut Desktop| &mut s.widgets.sidebar_pane,
            sidebar,
        )
        .toolbar(header::toolbar(s, t, &mut overlays))
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
    sidebar::overlays(s, t, &mut overlays);
    palette::mount(s, t, &mut overlays);
    overlays.extend(toasts(s, t));
    rune_kit::root(split, overlays).hover_motion_within(motion::INSTANT, motion::HOVER_OUT)
}

/// The toast stack, when there's anything in it.
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

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('k') && !ev.mods.shift && !ev.mods.alt {
        s.dispatch(Msg::Palette(PaletteMsg::Toggle), tx);
        return true;
    }
    if s.model.palette.open {
        return false;
    }
    if ev.mods.shift || ev.mods.alt {
        return false;
    }
    if ev.is_command('b') {
        s.dispatch(Msg::Sidebar(SidebarMsg::TogglePane), tx);
        return true;
    }
    if ev.is_command('[') {
        s.dispatch(Msg::Selection(SelectionMsg::GoBack), tx);
        return true;
    }
    if ev.is_command(']') {
        s.dispatch(Msg::Selection(SelectionMsg::GoForward), tx);
        return true;
    }
    if ev.is_command('n') {
        s.dispatch(Msg::Selection(SelectionMsg::CreateHere), tx);
        return true;
    }
    if ev.is_command('.') && s.model.busy() {
        s.dispatch(Msg::Composer(ComposerMsg::Abort), tx);
        return true;
    }
    false
}

fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let pane_changed = sync_sidebar_pane(s, tx);
    let completion = poll_completion(s, clock, tx);
    let palette = poll_palette(s, clock, tx);
    let tool_timer = if s.model.transcript().acp_partial.iter().any(|item| {
        matches!(item, model::Item::Tool(tool) if tool.status == model::ToolStatus::Running
            && tool.started_at.is_some() && tool.timeout_ms.is_some())
    }) {
        let now = theme::now_ms();
        tx.wake_after(Duration::from_millis(500));
        if now.saturating_sub(s.model.disclosure.tool_clock) >= 500 {
            s.dispatch(Msg::Timeline(TimelineMsg::TickToolClock(now)), tx);
            true
        } else {
            false
        }
    } else {
        false
    };
    s.expire_toasts(clock, tx) || pane_changed || completion || palette || tool_timer
}

fn sync_sidebar_pane(s: &mut Desktop, tx: &mut Tx) -> bool {
    if s.widgets.sidebar_pane.is_resizing() {
        return false;
    }
    let open = s.widgets.sidebar_pane.is_open();
    let width = s.widgets.sidebar_pane.width();
    let changed =
        s.model.panels.sidebar.open != open || (s.model.panels.sidebar.width - width).abs() >= 0.5;
    if changed {
        s.dispatch(Msg::Shell(ShellMsg::SidebarPaneChanged { open, width }), tx);
    }
    changed
}

fn poll_completion(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let before_token = s.model.composer.completion.token.clone();
    let before_due = s.model.composer.completion.due;
    let before_focused = s.model.composer.draft_focused;
    let cursor = s.draft_cursor();
    s.dispatch(
        Msg::Composer(ComposerMsg::PollCompletion { clock, cursor }),
        tx,
    );
    s.model.composer.completion.token != before_token
        || s.model.composer.completion.due != before_due
        || s.model.composer.draft_focused != before_focused
}

fn poll_palette(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    if !s.model.palette.open || s.model.palette.filter.is_empty() {
        return false;
    }
    let should_step = if s.model.connected() {
        matches!(s.model.palette.search, PaletteSearch::Idle)
            || matches!(s.model.palette.search, PaletteSearch::Debouncing(due) if clock >= due)
    } else {
        !matches!(s.model.palette.search, PaletteSearch::Idle)
    };
    if should_step {
        s.dispatch(Msg::Palette(PaletteMsg::Poll(clock)), tx);
    }
    should_step
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
    let initial_width = tooling.size.map_or(WINDOW_SIZE.0, |(width, _)| width);
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
        ..AppSpec::new("viz", WINDOW_SIZE, Desktop::detached, view)
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
        desktop.model.panels.window_width = initial_width;
        if !preview {
            desktop.restore_prefs();
        }
        desktop
    })
    .on_focus(terminal::scene::focus)
    .on_ime(terminal::scene::ime);
    if let Some(detail) = theme_error {
        app.handle().update(move |s, tx| {
            s.dispatch(
                Msg::Shell(ShellMsg::ShowToast {
                    title: "Invalid desktop configuration".into(),
                    detail: Some(detail),
                }),
                tx,
            );
        });
    }
    let ui = app.handle();
    let app = app.on_frame_drawn(move |rt| {
        let width = rt.window_viewport().0 / rt.zoom();
        if (rt.state.model.panels.window_width - width).abs() >= 1.0 {
            ui.update(move |s, tx| s.dispatch(Msg::Shell(ShellMsg::WindowResized(width)), tx));
        }
    });
    if !preview {
        let (first, connected) = std::sync::mpsc::channel();
        effects::server::spawn(app.handle(), receiver, first, endpoint);
        if headless {
            _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
        }
    }
    app.run();
}
