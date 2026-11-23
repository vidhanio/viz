//! Sidebar footer: environment switcher and connection status.

use rune::{El, col, icon, row, text};
use rune_kit::{ButtonVariant, Menu, MenuItem, Placement};
use viz_client::protocol::EnvironmentStatus;

use crate::{
    app::Desktop,
    model::Status,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, ROW_INSET, S6, S8, Size, Theme},
    ui,
    update::{Msg, SelectionMsg, SidebarMsg},
};

/// The quiet footer row: environment picker when more than one environment is
/// configured, merged with connection status. A healthy single-environment
/// connection shows no footer; Retry is omitted when the main pane already
/// offers it.
pub(super) fn footer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    let multi = m.connection.environments.len() > 1;
    let transcript = m.transcript();
    let main_has_retry = transcript.items.is_empty() && transcript.before.is_none() && !m.busy();
    let show_retry = matches!(m.connection.status, Status::Error(_)) && !main_has_retry;
    let content = if multi {
        environment_footer(s, t, layer)
    } else {
        status_footer(s, t)?
    };
    let mut row = row()
        .key("connection")
        .w_full()
        .h(Size::Default.height())
        .gap(S8)
        .center_y()
        .child(content);
    if show_retry {
        row = row.child(
            ui::button("Retry", ButtonVariant::GhostMuted, Size::Small)
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Selection(SelectionMsg::Reconnect), tx);
                })
                .build(&t.k)
                .key("retry"),
        );
    }
    Some(col().w_full().gap(S8).child(ui::rule(t)).child(row))
}

fn status_footer(s: &Desktop, t: &Theme) -> Option<El> {
    let m = &s.model;
    let (color, label) = match &m.connection.status {
        Status::Connecting => (t.warning, "Connecting…"),
        Status::Connected { .. } => return None,
        Status::Error(_) => (t.danger, "Offline"),
    };
    let detail = match (&m.connection.status, &m.connection.endpoint) {
        (Status::Error(message), _) => message.clone(),
        (_, Some(endpoint)) => endpoint.clone(),
        _ => String::new(),
    };
    Some(
        row()
            .role("status")
            .label(label)
            .help(detail)
            .pl(ROW_INSET)
            .gap(S8)
            .center_y()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .child(ui::lead(ui::dot(color), Size::Default.height()))
            .child(
                text(label)
                    .key("label")
                    .font(t.small)
                    .color(t.muted)
                    .truncate()
                    .shrink(),
            ),
    )
}

fn environment_footer(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
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
        .w_full()
        .h(Size::Default.height())
        .px(ROW_INSET)
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
                .font(t.ui_medium)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        )
        .children(note.map(|note| text(note).font(t.small).color(t.faint).a11y_hidden()))
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
