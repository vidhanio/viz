//! View for the keyboard command palette; all filtering, debouncing, and
//! command execution live in [`crate::model::palette`] and
//! [`crate::update::PaletteMsg`].

use rune::{Color, El, Icon, Key, KeyEvent, ScrollSpec, Span, Tx, col, icon, rich, row, text};
use rune_kit::{Keybind, TextInput};

use crate::{
    app::Desktop,
    model::{PaletteChoice, PaletteCommand, PaletteRow, PaletteSearch, palette_rows, thread_title},
    theme::{DIALOG_INSET, HAIRLINE, ICON, PALETTE_WIDTH, S4, S8, S16, S24, S32, Size, Theme},
    ui,
    update::{Msg, NavKey, PaletteMsg},
};

const FIELD: &str = "palette-query";
const LIST: &str = "palette-results";
/// The result list's height: eight one-line rows.
const LIST_HEIGHT: f32 = S32 * 8.0;

const fn command_glyph(command: PaletteCommand) -> &'static Icon {
    match command {
        PaletteCommand::NewThread => &ui::COMPOSE,
        PaletteCommand::AddProject => &ui::FOLDER_PLUS,
        PaletteCommand::Archive => &ui::ARCHIVE,
        PaletteCommand::Sidebar => &ui::SIDEBAR,
        PaletteCommand::Pin => &ui::PIN,
        PaletteCommand::Delete => &ui::TRASH,
        PaletteCommand::Stop => &ui::STOP,
    }
}

fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx, choices: &[PaletteChoice]) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.widgets.palette_query.preedit().is_some() {
        return false;
    }
    let key = match ev.key {
        Key::Down => NavKey::Down,
        Key::Up => NavKey::Up,
        Key::Enter => NavKey::Enter,
        _ => return false,
    };
    if choices.is_empty() {
        return true;
    }
    if key == NavKey::Enter && tx.key_repeat() {
        return true;
    }
    s.dispatch(
        Msg::Palette(PaletteMsg::Key {
            key,
            choices: choices.to_vec(),
        }),
        tx,
    );
    true
}

fn snippet(marked: &str, t: &Theme) -> El {
    let one_line = |ch: char| if ch.is_control() { ' ' } else { ch };
    let mut body = String::with_capacity(marked.len());
    let mut spans = Vec::new();
    let mut previous = 0;
    let mut start = None;
    for (index, marker) in marked.match_indices(['\u{1}', '\u{2}']) {
        body.extend(marked[previous..index].chars().map(one_line));
        previous = index + 1;
        if marker == "\u{1}" {
            start = Some(body.len());
        } else if let Some(start) = start.take().filter(|&start| start < body.len()) {
            spans.push(Span::new(start, body.len()).bold().color(t.accent_text));
        }
    }
    body.extend(marked[previous..].chars().map(one_line));
    rich(body, spans).font(t.small).color(t.muted).truncate()
}

fn result_row(result: PaletteRow<'_>, active: bool, s: &Desktop, t: &Theme) -> El {
    let m = &s.model;
    let choice = result.choice();
    let (glyph, title) = match result {
        PaletteRow::Command(command) => (command_glyph(command), command.label(m)),
        PaletteRow::Thread(thread) => (ui::harness_icon(&thread.harness), thread_title(thread)),
        PaletteRow::Message(hit) => (&ui::SEARCH, m.title(hit.thread)),
    };
    let label = text(title)
        .font(if active { t.ui_medium } else { t.ui })
        .color(if active { t.text } else { t.muted })
        .group_color(t.text)
        .truncate();
    let labels = col().grow(1.0).shrink().min_w(0.0).child(label);
    let labels = if let PaletteRow::Message(hit) = result {
        labels.child(snippet(&hit.snippet, t))
    } else {
        labels
    };
    let mut item = row()
        .key(choice.key())
        .role("option")
        .label(title)
        .focus_selected(active)
        .focusable(false)
        .preserve_focus_on_press(true)
        .w_full()
        .min_h(S32)
        .px(S8)
        .py(S4)
        .gap(S8)
        .center_y()
        .radius(Size::Default.radius())
        .fill(if active {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .hover_group();
    if active {
        item = item.focus_ring_for(FIELD, HAIRLINE, -HAIRLINE, t.accent);
    }
    item = item
        .child(
            icon(glyph, ICON)
                .color(if active { t.accent_text } else { t.muted })
                .group_color(if active { t.accent_text } else { t.text })
                .a11y_hidden(),
        )
        .child(labels)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Palette(PaletteMsg::Choose(choice)), tx);
        });
    if let PaletteRow::Thread(thread) = result
        && let Some(project) = thread
            .project
            .as_deref()
            .and_then(|id| m.cache().project(id))
    {
        item = item.child(
            text(&project.name)
                .font(t.small)
                .color(t.muted)
                .truncate()
                .shrink()
                .a11y_hidden(),
        );
    }
    if let PaletteRow::Command(command) = result
        && let Some(shortcut) = command.shortcut()
    {
        item = item.child(ui::kbd(shortcut, t));
    }
    item
}

fn search_note(s: &Desktop, any_choices: bool, t: &Theme) -> Option<El> {
    let m = &s.model;
    let (label, detail) = if !m.connected() {
        ("Connect to search threads and messages", None)
    } else if m.palette.filter.is_empty() {
        return None;
    } else {
        match &m.palette.search {
            PaletteSearch::Idle | PaletteSearch::Debouncing(_) | PaletteSearch::Pending => {
                ("Searching messages…", None)
            }
            PaletteSearch::Complete(Ok(hits)) if hits.is_empty() && !any_choices => {
                ("No results", None)
            }
            PaletteSearch::Complete(Err(error)) => ("Message search failed", Some(error)),
            PaletteSearch::Complete(Ok(_)) => return None,
        }
    };
    let hint = text(label)
        .role("status")
        .font(t.small)
        .color(t.faint)
        .px(S8)
        .py(S8);
    Some(if let Some(detail) = detail {
        hint.help(detail)
    } else {
        hint
    })
}

fn footer(t: &Theme) -> El {
    row()
        .w_full()
        .h(Size::Large.height())
        .px(DIALOG_INSET)
        .gap(S16)
        .center_y()
        .children(
            [
                (&["↑", "↓"][..], "Navigate"),
                (&["↵"][..], "Open"),
                (&["Esc"][..], "Close"),
            ]
            .map(|(keys, name)| {
                row()
                    .gap(S4)
                    .center_y()
                    .child(Keybind::new(keys).build(&t.overlay))
                    .child(text(name).font(t.small).color(t.faint))
            }),
        )
}

/// Mounts the searchable field and its result list in the shell overlay layer.
pub fn mount(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    if !m.palette.open {
        return;
    }
    let recent = m.palette.filter.is_empty();
    let results = palette_rows(m, recent);
    let active = m
        .palette
        .active
        .filter(|choice| results.iter().any(|result| result.choice() == *choice))
        .or_else(|| results.first().map(|result| result.choice()));
    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label("Palette results")
        .w_full()
        .max_h(LIST_HEIGHT)
        .min_h(0.0)
        .padding(S8)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S16);
    let mut choices = Vec::with_capacity(results.len());
    let mut group = "";
    for result in results {
        let name = result.group(recent);
        if name != group {
            group = name;
            list = list.child(
                row()
                    .key(name)
                    .h(S24)
                    .px(S8)
                    .center_y()
                    .child(ui::section(name, t)),
            );
        }
        let choice = result.choice();
        choices.push(choice);
        list = list.child(result_row(result, active == Some(choice), s, t));
    }
    if let Some(note) = search_note(s, !choices.is_empty(), t) {
        list = list.child(note.key("search-status"));
    }
    let field = TextInput::new(&s.widgets.palette_query)
        .label("Search commands, threads, and messages")
        .placeholder("Type a command or search threads…")
        .plain()
        .leading_icon(&ui::SEARCH)
        .width(None)
        .height(Size::Large.height())
        .active_descendant(active.map(PaletteChoice::key))
        .on_edit(|s: &mut Desktop| &mut s.widgets.palette_query)
        .on_change(|s: &mut Desktop, tx| {
            let text = s.widgets.palette_query.text.clone();
            s.dispatch(
                Msg::Palette(PaletteMsg::QueryChanged {
                    text,
                    clock: tx.clock(),
                }),
                tx,
            );
        })
        .on_key(move |s, ev, tx| on_key(s, ev, tx, &choices))
        .build(&t.overlay)
        .key(FIELD);
    ui::sheet(
        "palette",
        "Command palette",
        PALETTE_WIDTH,
        |s: &mut Desktop| s.dispatch_local(Msg::Palette(PaletteMsg::Close)),
        [
            row().w_full().px(S8).child(field),
            ui::rule(t),
            list,
            ui::rule(t),
            footer(t),
        ],
        t,
        layer,
    );
}
