//! Sidebar footer bar: environment switcher (when multiple environments are
//! configured) or connection status popover, quick project access, offline
//! retry button, and read-only shortcuts/config info popover.

use rune::{El, col, icon, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement, scrim};
use viz_client::protocol::EnvironmentStatus;

use crate::{
    app::Desktop,
    model::Status,
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, Size, Theme},
    ui,
    update::{Msg, SelectionMsg, SidebarMsg},
};

const INFO_TRIGGER_KEY: &str = "sidebar-info-trigger";
const FOOTER_HEIGHT: f32 = 40.0;

/// Clean `40px` footer bar anchored below the Settled drawer.
pub(super) fn footer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let multi = m.connection.environments.len() > 1;
    let transcript = m.transcript();
    let main_has_retry = transcript.items.is_empty() && transcript.before.is_none() && !m.busy();
    let show_retry = matches!(m.connection.status, Status::Error(_)) && !main_has_retry;

    let mut bar = row()
        .key("sidebar-footer-bar")
        .w_full()
        .h(FOOTER_HEIGHT)
        .px(S4)
        .gap(S4)
        .center_y();

    if multi {
        bar = bar.child(environment_switcher(s, t, layer));
    } else {
        bar = bar
            .child(single_env_status(s, t, layer))
            .child(rune::spacer());
    }

    if show_retry {
        bar = bar.child(
            ui::button("Retry", ButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Selection(SelectionMsg::Reconnect), tx);
                })
                .build(&t.k)
                .key("retry"),
        );
    }

    bar = bar.child(info_button(s, t, layer));

    col()
        .key("sidebar-footer")
        .w_full()
        .child(ui::rule(t).key("footer-rule"))
        .child(bar)
}

fn single_env_status(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let open = m.sidebar.env_menu;
    let (dot_color, label, detail) = connection_summary(s, t);
    let current = m.active_env_key();
    let display = if m.connected() { current } else { label };
    let machine = match m
        .connection
        .environments
        .iter()
        .find(|env| env.id == current)
        .map(|env| env.status)
    {
        Some(EnvironmentStatus::Local) | None => &ui::LAPTOP,
        Some(EnvironmentStatus::Reachable | EnvironmentStatus::Unreachable) => &ui::SERVER,
    };
    let trigger = row()
        .key("environment-status-trigger")
        .role("button")
        .label(format!("Environment: {current} ({label})"))
        .help(detail)
        .expanded(open)
        .h(Size::Compact.height())
        .px(S6)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .hover_group()
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleEnvMenu)))
        .child(ui::dot(dot_color))
        .child(
            icon(machine, ICON_SMALL + 1.0)
                .color(if open { t.text } else { t.icon_muted })
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(display)
                .font(t.small_medium)
                .color(if m.connected() { t.text } else { t.muted })
                .truncate()
                .a11y_hidden(),
        );

    let mut menu = Menu::new("environment-status-menu")
        .placement(Placement::TopStart)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseEnvMenu)))
        .trigger(trigger);
    if open {
        let mut item = MenuItem::new(current)
            .lead(ui::dot(dot_color))
            .checked(true);
        item = item.description(detail);
        menu = menu.item(item);
        if matches!(m.connection.status, Status::Error(_)) {
            menu = menu.separator().item(
                MenuItem::new("Reconnect")
                    .icon(&ui::REFRESH_CW)
                    .on_select_tx(|s: &mut Desktop, tx| {
                        s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseEnvMenu));
                        s.dispatch(Msg::Selection(SelectionMsg::Reconnect), tx);
                    }),
            );
        }
    }
    menu.build(&t.overlay, layer)
}

fn environment_switcher(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let m = &s.model;
    let open = m.sidebar.env_menu;
    let current = m
        .connection
        .environment
        .as_deref()
        .or_else(|| m.connection.environments.first().map(|env| env.id.as_str()))
        .unwrap_or("local");
    let active_env = m
        .connection
        .environments
        .iter()
        .find(|env| env.id == current);
    let (color, note, help) = match &m.connection.status {
        Status::Connecting => (
            t.warning,
            Some("Connecting…"),
            m.connection.endpoint.as_deref().unwrap_or("Connecting…"),
        ),
        Status::Error(message) => (t.danger, Some("Offline"), message.as_str()),
        Status::Connected { .. } => match active_env.map(|env| env.status) {
            Some(EnvironmentStatus::Unreachable) => (
                t.faint,
                Some("Unreachable"),
                active_env
                    .and_then(|env| env.url.as_deref())
                    .unwrap_or("Unreachable"),
            ),
            Some(EnvironmentStatus::Reachable) => (
                t.success,
                None,
                active_env
                    .and_then(|env| env.url.as_deref())
                    .unwrap_or("Reachable"),
            ),
            Some(EnvironmentStatus::Local) | None => (t.success, None, "Local"),
        },
    };
    let machine = match active_env.map(|env| env.status) {
        Some(EnvironmentStatus::Local) | None => &ui::LAPTOP,
        Some(EnvironmentStatus::Reachable | EnvironmentStatus::Unreachable) => &ui::SERVER,
    };
    let trigger = row()
        .key("environment-picker-trigger")
        .role("combobox")
        .label("Environment")
        .a11y_value(current)
        .help(help)
        .expanded(open)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .w_full()
        .h(Size::Compact.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .on_press(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleEnvMenu)))
        .child(ui::dot(color))
        .child(
            icon(machine, ICON_SMALL + 1.0)
                .color(if open { t.text } else { t.muted })
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(current)
                .font(t.small_medium)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        )
        .children(note.map(|note| text(note).font(t.micro).color(t.faint).a11y_hidden()))
        .child(
            icon(&ui::CHEVRONS_UP_DOWN, ICON_SMALL)
                .color(if open { t.text } else { t.faint })
                .group_color(t.text)
                .a11y_hidden(),
        );
    let mut menu = Menu::new("environment-picker")
        .placement(Placement::TopStart)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseEnvMenu)))
        .trigger(trigger);
    if open {
        menu = ui::menu_items(menu, environment_items(s, t));
    }
    menu.build(&t.overlay, layer).grow(1.0).shrink().min_w(0.0)
}

fn environment_items<'a>(s: &'a Desktop, t: &'a Theme) -> impl Iterator<Item = MenuItem> + 'a {
    let m = &s.model;
    m.connection.environments.iter().map(move |env| {
        let id = env.id.clone();
        let selected = m.connection.environment.as_deref() == Some(env.id.as_str());
        let (status_label, dot_color) = match env.status {
            EnvironmentStatus::Local => ("Local", t.success),
            EnvironmentStatus::Reachable => ("Reachable", t.success),
            EnvironmentStatus::Unreachable => ("Unreachable", t.faint),
        };
        let mut item = MenuItem::new(&env.id)
            .lead(ui::dot(dot_color))
            .checked(selected)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Selection(SelectionMsg::OpenInEnvironment {
                        environment: id.clone(),
                        thread: None,
                    }),
                    tx,
                );
            });
        if let Some(url) = &env.url {
            item = item.description(url);
        } else {
            item = item.description(status_label);
        }
        item
    })
}

fn info_button(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let open = s.model.sidebar.info_menu;
    let trigger = ui::icon_button(
        &ui::SETTINGS,
        "Shortcuts & config",
        IconButtonVariant::GhostMuted,
        Size::Compact,
    )
    .active(open)
    .on_press(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleInfoMenu)))
    .build(&t.k)
    .key(INFO_TRIGGER_KEY);

    if open {
        let close = |s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseInfoMenu));
        let shortcuts = [
            ("Command palette", ui::kbd("K", t)),
            ("Toggle sidebar", ui::kbd("B", t)),
            ("New thread", ui::kbd("N", t)),
            ("Jump to thread", ui::kbd("1–9", t)),
            ("Rename thread", ui::kbd_cap("F2", t)),
        ];
        let mut body = col().w_full().padding(S12).gap(S8).child(
            text("Keyboard shortcuts")
                .font(t.small_medium)
                .color(t.text_strong),
        );
        for (label, caps) in shortcuts {
            body = body.child(
                row()
                    .w_full()
                    .gap(S8)
                    .center_y()
                    .child(text(label).font(t.small).color(t.muted).grow(1.0))
                    .child(caps),
            );
        }
        body = body.child(ui::rule(t)).child(
            text("Process settings live in desktop.toml")
                .font(t.micro)
                .color(t.faint),
        );
        let frame = ui::popover_frame("sidebar-info-popover", 244.0, body, t)
            .role("dialog")
            .label("Shortcuts and configuration")
            .focus_return(INFO_TRIGGER_KEY)
            .modal(true, close);
        layer.push(scrim("sidebar-info-scrim", close));
        layer.push(motion::anchored_popover(
            frame,
            INFO_TRIGGER_KEY,
            Placement::TopEnd,
            S6,
        ));
    }
    trigger
}

fn connection_summary<'a>(s: &'a Desktop, t: &Theme) -> (rune::Color, &'static str, &'a str) {
    let m = &s.model;
    match &m.connection.status {
        Status::Connecting => (
            t.warning,
            "Connecting…",
            m.connection.endpoint.as_deref().unwrap_or("Connecting…"),
        ),
        Status::Connected { version } => (
            t.success,
            "Connected",
            m.connection.endpoint.as_deref().unwrap_or(version.as_str()),
        ),
        Status::Error(message) => (t.danger, "Offline", message.as_str()),
    }
}
