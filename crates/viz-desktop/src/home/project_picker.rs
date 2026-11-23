//! Searchable Home hero Project Picker combobox popover (`288px` flat
//! `combobox_shell` with pinned projects, hover/pinned pin toggles, remote
//! environment badge, `"No project"`, and `"Add project"` footer).

use rune::{El, Key, KeyEvent, Tx, icon, row, text};
use rune_kit::{Placement, scrim};

use super::PROJECT_TRIGGER_KEY;
use crate::{
    app::Desktop,
    model::{HomeProjectOption, home_project_options},
    theme::{HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_SM, S4, S6, S8, Theme},
    ui::{self, RowDensity},
    update::{ComposerMsg, HOME_PROJECT_SEARCH, Msg, NavKey, SelectionMsg},
};

/// Width of the Home hero Project Picker combobox popup (`w-72` = `288pt`).
const PROJECT_PICKER_WIDTH: f32 = 288.0;

/// Mounts the searchable Home hero Project Picker combobox popover anchored
/// beneath [`PROJECT_TRIGGER_KEY`].
pub(super) fn popover(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    let options = home_project_options(m);
    let active_idx = m
        .composer
        .home_project
        .highlighted
        .min(options.len().saturating_sub(1));
    let active_key = options.get(active_idx).map(|opt| opt.key());

    let search = rune_kit::TextInput::new(&s.widgets.home_project_query)
        .placeholder("Search projects...")
        .leading_icon(&ui::SEARCH)
        .width(None)
        .plain()
        .label("Search projects")
        .active_descendant(active_key)
        .on_edit(|s: &mut Desktop| &mut s.widgets.home_project_query)
        .on_change(|s: &mut Desktop, tx| {
            let query = s.widgets.home_project_query.text.clone();
            s.dispatch(
                Msg::Composer(ComposerMsg::HomeProjectQueryChanged(query)),
                tx,
            );
        })
        .on_key(on_project_key)
        .build(&t.k)
        .key(HOME_PROJECT_SEARCH);

    let current_path = m.project_path();
    let remote_env = m
        .connection
        .environment
        .as_deref()
        .filter(|env| *env != "local");

    let items: Vec<El> = options
        .into_iter()
        .enumerate()
        .map(|(index, option)| {
            picker_row(s, option, current_path, remote_env, index == active_idx, t)
        })
        .collect();

    let empty = text("No matching projects.")
        .font(t.small)
        .color(t.faint)
        .py(S8)
        .role("status");

    let footer = row()
        .key("home-project-add")
        .role("button")
        .label("Add project")
        .w_full()
        .h(28.0)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_SM)
        .hover_group()
        .hover(t.selected)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::CloseHomeProjectPicker), tx);
            s.dispatch(
                Msg::Palette(crate::update::PaletteMsg::OpenProject(
                    crate::model::projects::ProjectView::Sources,
                )),
                tx,
            );
        })
        .child(
            icon(&ui::FOLDER_PLUS, ICON_MD)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text("Add project")
                .font(t.panel_row)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        );

    let shell = ui::combobox_shell("home-project-picker")
        .width(PROJECT_PICKER_WIDTH)
        .max_height(240.0)
        .search(search)
        .items(items)
        .empty(empty)
        .footer(footer)
        .build(t)
        .role("dialog")
        .label("Choose project")
        .focus_return(PROJECT_TRIGGER_KEY)
        .modal(true, close_project_picker);

    layer.push(scrim("home-project-scrim", close_project_picker));
    layer.push(rune_kit::popover::anchored(
        &t.overlay,
        shell,
        PROJECT_TRIGGER_KEY,
        Placement::Bottom,
        S6,
    ));
}

fn picker_row(
    s: &Desktop,
    option: HomeProjectOption<'_>,
    current_path: Option<&str>,
    remote_env: Option<&str>,
    highlighted: bool,
    t: &Theme,
) -> El {
    match option {
        HomeProjectOption::NoProject => {
            let selected = current_path.is_none();
            let mut item = ui::list_row(option.key(), "No project")
                .density(RowDensity::Compact)
                .leading(
                    icon(&ui::MESSAGE_SQUARE_DASHED, ICON_MD)
                        .color(if selected { t.accent_text } else { t.faint })
                        .a11y_hidden(),
                )
                .selected(selected)
                .highlighted(highlighted)
                .focus_ring_for(HOME_PROJECT_SEARCH)
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Selection(SelectionMsg::ChooseDraftProject(None)), tx);
                });
            if selected {
                item = item.trailing(
                    icon(&ui::CHECK, ICON_SMALL)
                        .color(t.accent_text)
                        .a11y_hidden(),
                );
            }
            item.build(t)
        }
        HomeProjectOption::Project(project) => {
            let selected = current_path == Some(project.path.as_str());
            let pinned = project.settings.pinned;
            let select_path = project.path.clone();
            let pin_path = project.path.clone();

            let mut trailing = row().gap(S4).center_y();
            if let Some(env) = remote_env {
                trailing = trailing.child(
                    rune_kit::Badge::new(env)
                        .variant(rune_kit::BadgeVariant::Subtle)
                        .build(&t.k)
                        .a11y_hidden(),
                );
            }
            if selected {
                trailing = trailing.child(
                    icon(&ui::CHECK, ICON_SMALL)
                        .color(t.accent_text)
                        .a11y_hidden(),
                );
            }
            let pin_btn = row()
                .key(format!("pin-{}", project.path))
                .role("button")
                .label(if pinned {
                    format!("Unpin {}", project.name)
                } else {
                    format!("Pin {}", project.name)
                })
                .help(if pinned {
                    "Unpin project"
                } else {
                    "Pin project"
                })
                .size(20.0, 20.0)
                .center()
                .radius(RADIUS_SM)
                .hover(t.hover_wash)
                .focusable(true)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .group_opacity(if pinned { 1.0 } else { 0.0 }, 1.0)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Composer(ComposerMsg::ToggleProjectPin(pin_path.clone())),
                        tx,
                    );
                })
                .child(
                    icon(&ui::PIN, ICON_SMALL)
                        .color(if pinned { t.accent_text } else { t.faint })
                        .a11y_hidden(),
                );
            trailing = trailing.child(pin_btn);

            ui::list_row(option.key(), &project.name)
                .density(RowDensity::Compact)
                .leading(ui::project_mark(s, Some(project), 16.0, t))
                .selected(selected)
                .highlighted(highlighted)
                .focus_ring_for(HOME_PROJECT_SEARCH)
                .trailing(trailing)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Selection(SelectionMsg::ChooseDraftProject(Some(select_path.clone()))),
                        tx,
                    );
                })
                .build(t)
        }
    }
}

fn close_project_picker(s: &mut Desktop) {
    s.dispatch_local(Msg::Composer(ComposerMsg::CloseHomeProjectPicker));
}

fn on_project_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.widgets.home_project_query.preedit().is_some() {
        return false;
    }
    let key = match ev.key {
        Key::Down => NavKey::Down,
        Key::Up => NavKey::Up,
        Key::Enter => NavKey::Enter,
        Key::Escape => NavKey::Escape,
        _ => return false,
    };
    s.dispatch(Msg::Composer(ComposerMsg::HomeProjectKey(key)), tx);
    true
}
