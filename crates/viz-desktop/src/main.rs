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
//! - Layout: shell slots for sidebar, main (timeline + composer), details,
//!   diff, and overlays (menus, palette, dialogs, toasts).
//! - Visuals come from `theme` tokens and `ui` components, derived strictly
//!   from the configured base16 scheme in a flat, sans-serif style with crisp
//!   hairline borders and no blur, glow, or drop shadows. Icons are Lucide plus
//!   monochrome harness marks.
//! - Motion: tokens and reusable primitives live in `motion`. Every spring is
//!   critically damped (zero bounce) and preserves velocity on mid-flight
//!   interruption; Reduce Motion removes spatial travel and stops live loops.

mod app;
mod attachments;
mod git;
use git::toolbar as checkout;
mod composer;
mod config;
mod effects;
mod header;
mod home;
mod model;
mod model_picker;
mod motion;
mod motion_scene;
mod palette;
mod projects;
mod requests;
mod scenes;
mod shell;
mod sidebar;
mod theme;
mod timeline;
mod ui;
mod update;

use std::{path::PathBuf, time::Duration};

use clap::Parser;
use rune::{App, AppSpec, Chrome, Cx, El, ImageCache, Key, KeyEvent, Tx, col};
use tokio::sync::mpsc;
use viz_client::Endpoint;
use viz_config::Component;

use crate::{
    app::Desktop,
    config::DesktopConfig,
    model::{PaletteSearch, WINDOW_SIZE},
    shell::ShellSlots,
    theme::Theme,
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
    if let Some(demo) = &s.motion_demo {
        return motion_scene::view(demo, reduced, t);
    }
    let mut overlays = Vec::new();
    let main = if home::is_active(&s.model) {
        col()
            .key("main")
            .w_full()
            .h_full()
            .min_h(0.0)
            .child(home::view(s, t, &mut overlays))
    } else {
        let dock = composer::composer(s, t, &mut overlays);
        let approvals: Vec<El> = requests::approval_rows(s, t).collect();
        let pending_requests = (!approvals.is_empty()).then(|| {
            col()
                .key("approval-rows")
                .w_full()
                .gap(theme::S8)
                .children(approvals)
        });
        let mut main = col()
            .key("main")
            .w_full()
            .h_full()
            .min_h(0.0)
            .child(timeline::timeline(
                s,
                timeline::TimelineSlots { pending_requests },
                t,
                &mut overlays,
            ));
        if let Some(dock) = dock {
            main = main.child(dock);
        }
        if let Some(card) = git::details_card::card(s, t, &mut overlays) {
            main = main.child(card);
        }
        main
    };
    let sidebar = sidebar::sidebar(s, cx.titlebar_inset(), t, &mut overlays);
    let toolbar = header::toolbar(s, t);
    let diff = (s.model.git.status.is_some() || s.model.panels.diff.open)
        .then(|| git::diff_panel::view(s, t, &mut overlays));
    git::dialogs(s, t, &mut overlays);
    sidebar::overlays(s, t, &mut overlays);
    palette::mount(s, t, &mut overlays);
    composer::attachments::lightbox(s, reduced, t, &mut overlays);
    let slots = ShellSlots {
        sidebar,
        toolbar,
        main,
        diff,
        overlays,
    };
    shell::build(s, slots, cx.titlebar_inset(), reduced, t)
}

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('k') && !ev.mods.shift && !ev.mods.alt {
        s.dispatch(Msg::Palette(PaletteMsg::Toggle), tx);
        return true;
    }
    if s.model.palette.open {
        return false;
    }
    if model_picker::shortcut(s, ev, tx) {
        return true;
    }
    if ev.is_command('p') && ev.mods.shift && !ev.mods.alt && home::is_active(&s.model) {
        s.dispatch(Msg::Composer(ComposerMsg::ToggleHomeProjectPicker), tx);
        return true;
    }
    if ev.is_command('n') && ev.mods.alt && !ev.mods.shift && s.model.connected() {
        s.dispatch(Msg::Selection(SelectionMsg::StartWithoutProject), tx);
        return true;
    }
    if ev.mods.shift || ev.mods.alt {
        return false;
    }
    if ev.is_command('d') && s.model.git.status.is_some() {
        git::dispatch(s, update::GitMsg::ToggleDiff, tx);
        return true;
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
        s.dispatch(Msg::Sidebar(SidebarMsg::NewThread), tx);
        return true;
    }
    if ev.is_command('.') && s.model.busy() {
        s.dispatch(Msg::Composer(ComposerMsg::Abort), tx);
        return true;
    }
    for digit in 1_u8..=9 {
        let ch = char::from(b'0' + digit);
        if ev.is_command(ch) {
            s.dispatch(
                Msg::Sidebar(SidebarMsg::JumpToIndex {
                    index: usize::from(digit - 1),
                    now: theme::now_ms(),
                }),
                tx,
            );
            return true;
        }
    }
    if ev.key == Key::Function(2)
        && !ev.mods.ctrl
        && !ev.mods.cmd
        && s.model.sidebar.edit.is_none()
        && let Some(id) = s.model.selection.selected
    {
        s.dispatch(Msg::Sidebar(SidebarMsg::StartInlineRename(id)), tx);
        return true;
    }
    if ev.key == Key::Escape
        && (s.model.sidebar.inline_rename.is_some()
            || s.model.sidebar.drag.is_some()
            || s.model.sidebar.filter_open
            || s.model.sidebar.snooze_popover.is_some()
            || s.model.sidebar.info_menu
            || s.model.sidebar.hover_card.is_some())
    {
        s.dispatch(Msg::Sidebar(SidebarMsg::DismissTransient), tx);
        return true;
    }
    false
}

fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let pane_changed = sync_sidebar_pane(s, tx);
    let diff_width = s.widgets.diff_pane.width();
    let diff_resized =
        !s.widgets.diff_pane.is_resizing() && (s.model.panels.diff.width - diff_width).abs() >= 0.5;
    if diff_resized {
        git::dispatch(s, update::GitMsg::PaneWidth(diff_width), tx);
    }
    let zoom = s.widgets.image_zoom.poll(clock, tx);
    let math = s.widgets.math.changed();
    let completion = poll_completion(s, clock, tx);
    let palette = poll_palette(s, clock, tx);
    let history = poll_earlier_history(s, tx);
    let has_tool_timeout = s.model.transcript().acp_partial.iter().any(|item| {
        matches!(item, model::Item::Tool(tool) if tool.status == model::ToolStatus::Running
            && tool.started_at.is_some() && tool.timeout_ms.is_some())
    });
    let tick_ms: Option<i64> = if has_tool_timeout {
        Some(500)
    } else if s.model.busy() {
        Some(1_000)
    } else {
        None
    };
    let tool_timer = if let Some(interval_ms) = tick_ms {
        let now = theme::now_ms();
        tx.wake_after(Duration::from_millis(
            u64::try_from(interval_ms).unwrap_or(1_000),
        ));
        if now.saturating_sub(s.model.disclosure.tool_clock) >= interval_ms {
            s.dispatch(Msg::Timeline(TimelineMsg::TickToolClock(now)), tx);
            true
        } else {
            false
        }
    } else {
        false
    };
    s.expire_toasts(clock, tx)
        || pane_changed
        || diff_resized
        || zoom
        || math
        || completion
        || palette
        || history
        || tool_timer
}

/// Automatically loads the previous transcript page as the user scrolls near
/// the top of a thread with older history.
fn poll_earlier_history(s: &mut Desktop, tx: &mut Tx) -> bool {
    const AUTO_LOAD_TOP_PX: f32 = 280.0;
    let page = s.model.timeline_page_key();
    if s.widgets.timeline_scroll_page != page {
        s.widgets.timeline_scroll_page = page;
        s.widgets.timeline_scroll.set([f32::MAX, 0.0]);
        return false;
    }
    let [offset_y, max_y] = s.widgets.timeline_scroll.get();
    if s.model.connected()
        && s.model.transcript().before.is_some()
        && !s.model.is_history_loading()
        && max_y > 1.0
        && offset_y <= AUTO_LOAD_TOP_PX
    {
        s.dispatch(Msg::Timeline(TimelineMsg::LoadEarlier), tx);
        return true;
    }
    false
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
    let token = (s.model.connected() && s.model.composer.open_picker.is_none())
        .then(|| {
            model::completion_token_at(
                &s.widgets.draft.text,
                &cursor,
                &s.model.composer.draft.pills,
            )
        })
        .flatten();
    if token == before_token
        && cursor.focused == before_focused
        && before_due.is_none_or(|due| clock < due)
    {
        return false;
    }
    s.dispatch(
        Msg::Composer(ComposerMsg::PollCompletion { clock, cursor }),
        tx,
    );
    s.model.composer.completion.token != before_token
        || s.model.composer.completion.due != before_due
        || s.model.composer.draft_focused != before_focused
}

fn poll_palette(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    if !s.model.palette.pages.stack.is_empty() {
        if s.model.palette.pages.due.is_some_and(|due| clock >= due) {
            s.dispatch(Msg::Palette(PaletteMsg::Poll(clock)), tx);
            return true;
        }
        return false;
    }
    if !s.model.palette.open
        || s.model.palette.filter.is_empty()
        || s.model.palette.filter.starts_with('>')
    {
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
    /// Use this config file instead of `$VIZ_DESKTOP_CONFIG` or the default
    /// location.
    #[arg(long)]
    config: Option<PathBuf>,
}

/// Strip viz-specific flags before Rune handles its own tooling arguments.
fn args() -> (Option<PathBuf>, Vec<String>) {
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
    (args.config, rune)
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
    let initial_height = tooling.size.map_or(WINDOW_SIZE.1, |(_, height)| height);
    let scene_theme = preview
        .then(|| std::env::var("VIZ_SCENE_THEME").ok())
        .flatten();
    let (theme_error, endpoint) =
        match Component::Desktop.load_client::<DesktopConfig>(file.as_deref()) {
            Ok((client, config)) => {
                let endpoint = Endpoint::from_connection(&client.server).map_err(|e| e.to_string());
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
        setup: Some(|s, waker| {
            s.widgets.math.set_waker(waker.clone());
            s.widgets.images = Some(ImageCache::new(waker));
        }),
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
        desktop.model.panels.window_height = initial_height;
        if !preview {
            desktop.restore_prefs();
        }
        desktop
    })
    .on_focus(Desktop::set_focus)
    .compositor(motion::FxCompositor::default());
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
    if !preview {
        effects::platform::install_notifications(app.handle());
        let (first, connected) = std::sync::mpsc::channel();
        effects::server::spawn(app.handle(), receiver, first, endpoint);
        if headless {
            _ = connected.recv_timeout(HEADLESS_CONNECT_WAIT);
        }
    }
    app.run();
}
