//! Nested command palette pages and keyboard navigation.

use rune::{El, Key, ScrollSpec, col, row, text};

use super::{FIELD, LIST, LIST_HEIGHT, query_input};
use crate::{
    app::Desktop,
    model::projects::ProjectAction,
    projects,
    theme::{PALETTE_WIDTH, S8, S16, Size, Theme},
    ui,
    update::{Msg, PaletteMsg},
};

pub(super) fn mount(
    s: &Desktop,
    page: &crate::model::projects::ProjectPage,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    use crate::{
        model::projects::{ProjectReply, ProjectView},
        motion::{Switch, TRAVEL_TITLE},
    };
    let options = projects::options(&s.model, page);
    let choices: Vec<_> = options
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row.action().map(|action| (i, action)))
        .collect();
    let active = page
        .active
        .filter(|active| choices.iter().any(|(i, _)| i == active));
    let count = options.len();
    let grouped = options.iter().any(|item| item.section_label().is_some());
    let list_children: Vec<El> = if grouped {
        grouped_option_cards(options, active, t)
    } else {
        options
            .into_iter()
            .enumerate()
            .map(|(i, option)| option.build(i, Some(i) == active, t))
            .collect()
    };
    let list = col()
        .key(LIST)
        .role("listbox")
        .label(page.view.title())
        .max_h(LIST_HEIGHT)
        .min_h(0.0)
        .w_full()
        .padding(S8)
        .gap(if grouped { crate::theme::S12 } else { 0.0 })
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S16)
        .instant_changes()
        .children(list_children);
    let submit = projects::submit(page)
        .filter(|_| !page.pending)
        .map(|(_, action)| action);
    let completion = match &page.data {
        Some(ProjectReply::Folders(data)) if !page.pending => data.completion.clone(),
        _ => None,
    };
    let is_folder = matches!(page.view, ProjectView::Folders(_));
    let no_input = matches!(page.view, ProjectView::Remove(_) | ProjectView::Cloning);
    let field = project_input(
        s,
        page.view.placeholder(),
        ProjectInput {
            choices,
            active,
            submit,
            completion,
            is_folder,
            input_disabled: page.write_pending,
        },
        t,
    );
    let mut view = col()
        .gap(S8)
        .pb(S8)
        .child(header(page.view.title(), t))
        .child(ui::rule(t));
    if !no_input {
        view = view.child(row().px(S8).child(field));
    }
    view = view.child(projects::context(s, page, count, t));
    if count > 0 {
        view = view.child(list);
    }
    if let Some(footer) = projects::footer(page, t) {
        view = view.child(row().px(S16).child(footer));
    }
    if !no_input {
        view = view.child(ui::rule(t)).child(legend(is_folder, t));
    }
    let pages = &s.model.palette.pages;
    let switch = if pages.backwards {
        Switch::Out
    } else {
        Switch::Into
    };
    ui::sheet(
        "palette",
        "Command palette",
        PALETTE_WIDTH,
        |s: &mut Desktop| s.dispatch_local(Msg::Palette(PaletteMsg::Close)),
        [col().child_exit(switch.view_exit(TRAVEL_TITLE)).child(
            view.key(format!("palette-view-{}", pages.revision))
                .transition(switch.view_enter(TRAVEL_TITLE)),
        )],
        t,
        layer,
    );
}

struct ProjectInput {
    choices: Vec<(usize, ProjectAction)>,
    active: Option<usize>,
    submit: Option<ProjectAction>,
    completion: Option<String>,
    is_folder: bool,
    input_disabled: bool,
}

fn project_input(s: &Desktop, placeholder: &str, input: ProjectInput, t: &Theme) -> El {
    let ProjectInput {
        choices,
        active,
        submit,
        completion,
        is_folder,
        input_disabled,
    } = input;
    query_input(s, placeholder)
        .plain()
        .disabled(input_disabled)
        .active_descendant(active.map(projects::option_key))
        .on_key(move |s: &mut Desktop, ev, tx| {
            if s.widgets.palette_query.preedit().is_some() {
                return false;
            }
            let action = match ev.key {
                Key::Backspace if s.widgets.palette_query.text.is_empty() => Some(PaletteMsg::Back),
                Key::Enter if !tx.key_repeat() => {
                    let highlighted = (!ev.mods.ctrl && !ev.mods.cmd)
                        .then(|| choices.iter().find(|(i, _)| Some(*i) == active))
                        .flatten();
                    highlighted
                        .map(|(_, action)| action.clone())
                        .or_else(|| submit.clone())
                        .or_else(|| choices.first().map(|(_, action)| action.clone()))
                        .map(PaletteMsg::Project)
                }
                Key::Tab if is_folder && !ev.mods.shift => {
                    let folder = choices
                        .iter()
                        .find(|(i, _)| Some(*i) == active)
                        .map(|(_, action)| action.clone());
                    folder
                        .or_else(|| completion.clone().map(ProjectAction::Browse))
                        .map(PaletteMsg::Project)
                }
                Key::Up | Key::Down if !choices.is_empty() => {
                    let n = choices.len();
                    let index =
                        active.and_then(|active| choices.iter().position(|(i, _)| *i == active));
                    let down = ev.key == Key::Down;
                    let next = index.map_or(if down { 0 } else { n - 1 }, |i| {
                        if down { (i + 1) % n } else { (i + n - 1) % n }
                    });
                    Some(PaletteMsg::Highlight(choices[next].0))
                }
                _ => None,
            };
            action.is_some_and(|action| {
                s.dispatch(Msg::Palette(action), tx);
                true
            })
        })
        .build(&t.overlay)
        .key(FIELD)
}

fn legend(folder: bool, t: &Theme) -> El {
    let mut hints: Vec<(&[&str], &str)> = vec![
        (&["↑", "↓"], "Navigate"),
        (&["↵"], "Select"),
        (&["⌫"], "Back"),
    ];
    if folder {
        hints.push((&["Tab"], "Complete"));
    }
    hints.push((&["Esc"], "Close"));
    super::key_hints(hints, t)
}

type OptionGroup = (Option<&'static str>, Vec<(usize, projects::OptionRow)>);

fn grouped_option_cards(
    options: Vec<projects::OptionRow>,
    active: Option<usize>,
    t: &Theme,
) -> Vec<El> {
    let mut groups: Vec<OptionGroup> = Vec::new();
    for (i, option) in options.into_iter().enumerate() {
        let section = option.section_label();
        if let Some((last_section, items)) = groups.last_mut()
            && *last_section == section
        {
            items.push((i, option));
        } else {
            groups.push((section, vec![(i, option)]));
        }
    }
    let mut out = Vec::with_capacity(groups.len());
    for (g, (section, items)) in groups.into_iter().enumerate() {
        let len = items.len();
        let mut card = col()
            .key(format!("settings-card-{g}"))
            .w_full()
            .padding(crate::theme::S4)
            .radius(crate::theme::RADIUS_CARD)
            .fill(t.surface)
            .border(crate::theme::HAIRLINE, t.hairline);
        for (pos, (index, option)) in items.into_iter().enumerate() {
            card = card.child(option.build(index, Some(index) == active, t));
            if pos + 1 < len {
                card = card.child(
                    rune::el()
                        .key(format!("settings-sep-{g}-{pos}"))
                        .h(0.5)
                        .ml(crate::theme::S12)
                        .mr(crate::theme::S12)
                        .fill(t.hairline)
                        .a11y_hidden(),
                );
            }
        }
        let mut wrapper = col()
            .key(format!("settings-group-{g}"))
            .w_full()
            .gap(crate::theme::S4);
        if let Some(title) = section {
            wrapper = wrapper.child(
                text(title)
                    .font(t.micro_medium)
                    .color(t.muted)
                    .ml(crate::theme::S12),
            );
        }
        out.push(wrapper.child(card));
    }
    out
}

fn header(title: &str, t: &Theme) -> El {
    let back = rune_kit::IconButton::new(&ui::ARROW_LEFT)
        .help("Back")
        .variant(rune_kit::IconButtonVariant::Ghost)
        .control_size(Size::Compact.kit())
        .on_press_tx(|s: &mut Desktop, tx| s.dispatch(Msg::Palette(PaletteMsg::Back), tx))
        .build(&t.overlay);
    row()
        .px(S8)
        .gap(S8)
        .h(Size::Large.height())
        .center_y()
        .child(back)
        .child(text(title).font(t.ui_medium).color(t.text))
}
