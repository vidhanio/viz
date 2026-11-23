//! Project Filter Combobox anchored below the sidebar filter button.
//!
//! Lists `"All projects"`, pinned projects first (sorted alphabetically),
//! remaining registered projects, and `"No project"`, with search filtering,
//! thread counts, project monograms/icons, and keyboard navigation.

use rune::{Align, El, Key, KeyEvent, Side, Tx, icon, text};
use rune_kit::scrim;

use crate::{
    app::Desktop,
    model::{ProjectFilter, ProjectFilterOption, project_filter_options},
    motion,
    theme::{ICON_MD, ICON_SMALL, S6, Theme},
    ui::{self, RowDensity},
    update::{Msg, NavKey, PROJECT_FILTER_INPUT, SidebarMsg},
};

/// Element key of the Row 2 filter trigger button used as the combobox anchor.
pub(super) const FILTER_TRIGGER_KEY: &str = "sidebar-project-filter-trigger";
const COMBOBOX_WIDTH: f32 = 248.0;

/// Mounts the searchable project filter combobox into `layer` while open.
pub(super) fn combobox(s: &Desktop, now: i64, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    if !m.sidebar.filter_open {
        return;
    }
    let options = project_filter_options(m, now);
    let active_idx = m.sidebar.filter_active.min(options.len().saturating_sub(1));
    let active_key = options.get(active_idx).map(|opt| opt.key());

    let search = {
        let input = rune_kit::TextInput::new(&s.widgets.project_filter_query)
            .placeholder("Search projects…")
            .leading_icon(&ui::SEARCH)
            .width(None)
            .plain()
            .label("Filter by project")
            .active_descendant(active_key)
            .on_edit(|s: &mut Desktop| &mut s.widgets.project_filter_query)
            .on_change(|s: &mut Desktop, tx: &mut Tx| {
                let query = s.widgets.project_filter_query.text.clone();
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::ProjectFilterQueryChanged(query)),
                    tx,
                );
            })
            .on_key(move |s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx| {
                on_filter_key(s, ev, now, tx)
            });
        if s.widgets.project_filter_query.text.is_empty() {
            input
        } else {
            input
                .clear_button()
                .on_action(|s: &mut Desktop, tx: &mut Tx| {
                    s.widgets.project_filter_query.clear();
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::ProjectFilterQueryChanged(String::new())),
                        tx,
                    );
                })
        }
    }
    .build(&t.k)
    .key(PROJECT_FILTER_INPUT);

    let items: Vec<El> = options
        .into_iter()
        .enumerate()
        .map(|(index, option)| filter_row(s, option, index == active_idx, t))
        .collect();

    let empty = text("No matching projects")
        .font(t.small)
        .color(t.faint)
        .role("status");

    let shell = ui::combobox_shell("project-filter-combobox")
        .width(COMBOBOX_WIDTH)
        .max_height(280.0)
        .search(search)
        .items(items)
        .empty(empty)
        .build(t)
        .role("dialog")
        .label("Filter by project")
        .focus_return(FILTER_TRIGGER_KEY)
        .modal(true, close);

    layer.push(scrim("project-filter-scrim", close));
    layer.push(
        shell
            .anchored_fit(FILTER_TRIGGER_KEY, Side::Bottom, Align::Start, S6)
            .transition(motion::fade(motion::QUICK_PAIR)),
    );
}

fn close(s: &mut Desktop) {
    s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseProjectFilter));
}

fn on_filter_key(s: &mut Desktop, ev: &KeyEvent, now: i64, tx: &mut Tx) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.widgets.project_filter_query.preedit().is_some() {
        return false;
    }
    let key = match ev.key {
        Key::Down => NavKey::Down,
        Key::Up => NavKey::Up,
        Key::Enter => NavKey::Enter,
        Key::Escape => NavKey::Escape,
        _ => return false,
    };
    if key == NavKey::Enter {
        tx.instant();
    }
    s.dispatch(Msg::Sidebar(SidebarMsg::ProjectFilterKey { key, now }), tx);
    true
}

fn filter_row(s: &Desktop, option: ProjectFilterOption<'_>, highlighted: bool, t: &Theme) -> El {
    let target = option.to_filter();
    let selected = s.model.sidebar.project_filter == target;
    let key = option.key();
    let (label, count, leading) = match option {
        ProjectFilterOption::All { count } => (
            "All projects",
            count,
            icon(&ui::FOLDER, ICON_MD)
                .color(if selected { t.accent_text } else { t.muted })
                .a11y_hidden(),
        ),
        ProjectFilterOption::Project { project, count } => (
            project.name.as_str(),
            count,
            ui::project_mark(s, Some(project), 16.0, t),
        ),
        ProjectFilterOption::NoProject { count } => (
            "No project",
            count,
            icon(&ui::MESSAGE_SQUARE_DASHED, ICON_MD)
                .color(if selected { t.accent_text } else { t.faint })
                .a11y_hidden(),
        ),
    };

    let mut row = ui::list_row(key, label)
        .density(RowDensity::Compact)
        .leading(leading)
        .selected(selected)
        .highlighted(highlighted)
        .focus_ring_for(PROJECT_FILTER_INPUT)
        .on_press_tx(move |s: &mut Desktop, tx| {
            tx.instant();
            s.dispatch(
                Msg::Sidebar(SidebarMsg::SetProjectFilter(target.clone())),
                tx,
            );
        });
    if count > 0 {
        row = row.detail(count.to_string());
    }
    if selected {
        row = row.trailing(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.accent_text)
                .a11y_hidden(),
        );
    }
    row.build(t)
}

/// Display label and leading mark for the conditional active project filter
/// chip shown below Row 2.
pub(super) fn active_chip_parts<'a>(s: &'a Desktop, t: &Theme) -> Option<(El, &'a str)> {
    match &s.model.sidebar.project_filter {
        ProjectFilter::All => None,
        ProjectFilter::NoProject => Some((ui::project_mark(s, None, ICON_MD, t), "No project")),
        ProjectFilter::Project(path) => {
            let project = s.model.cache().project(path);
            let name = project.map_or(path.as_str(), |p| p.name.as_str());
            Some((ui::project_mark(s, project, ICON_MD, t), name))
        }
    }
}
