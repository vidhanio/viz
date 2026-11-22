use std::{ops::Range, time::Duration};

use rune::{Color, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use viz_client::protocol::SlashCommand;

use crate::{
    app::Desktop,
    motion,
    server::Command,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, S16, S32, Theme},
    ui,
};

const FILE_DELAY: Duration = Duration::from_millis(110);
const POPUP: &str = "composer-completion";
const LIST: &str = "completion-list";
/// Maximum scrollable height of the completion drawer before scrolling.
const DRAWER_MAX_HEIGHT: f32 = 248.0;
/// Micro-header height at the top of the completion drawer.
const HEADER_HEIGHT: f32 = 26.0;
/// Maximum width of a slash command's argument hint.
const HINT_MAX_WIDTH: f32 = 140.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Slash,
    File,
}

#[derive(Debug, PartialEq, Eq)]
struct Token<Q> {
    kind: Kind,
    range: Range<usize>,
    query: Q,
}

impl<'a> Token<&'a str> {
    fn at(edit: &'a TextEdit) -> Option<Self> {
        if !edit.focused || edit.anchor != edit.focus {
            return None;
        }
        let before = edit.text.get(..edit.focus)?;
        let word = before.rsplit(char::is_whitespace).next()?;
        let start = before.len() - word.len();
        let after = &edit.text[edit.focus..];
        let end = edit.focus + after.find(char::is_whitespace).unwrap_or(after.len());
        let (kind, query) = match word.as_bytes().first().copied() {
            Some(b'/') if start == 0 => (Kind::Slash, &word[1..]),
            Some(b'@') if !word.starts_with("@\"") => (Kind::File, &word[1..]),
            _ => return None,
        };
        Some(Self {
            kind,
            range: start..end,
            query,
        })
    }
}

#[derive(Debug, Default)]
pub struct Completion {
    token: Option<Token<String>>,
    request: u64,
    due: Option<f64>,
    files: Option<viz_client::Result<Vec<String>>>,
    highlighted: usize,
    dismissed: bool,
}

impl Completion {
    pub fn reset(&mut self) {
        self.token = None;
        self.request += 1;
        self.due = None;
        self.files = None;
        self.highlighted = 0;
        self.dismissed = false;
    }

    pub fn file_result(&mut self, request: u64, result: viz_client::Result<Vec<String>>) {
        if request == self.request {
            self.files = Some(result);
        }
    }

    /// Seeds a slash-command completion state for preview scenes.
    pub fn preview_slash(&mut self, query: &str, highlighted: usize) {
        self.reset();
        self.token = Some(Token {
            kind: Kind::Slash,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.highlighted = highlighted;
    }

    /// Seeds a file-mention completion state for preview scenes.
    pub fn preview_files(&mut self, query: &str, paths: Vec<String>, highlighted: usize) {
        self.reset();
        self.token = Some(Token {
            kind: Kind::File,
            range: 0..query.len() + 1,
            query: query.to_owned(),
        });
        self.files = Some(Ok(paths));
        self.highlighted = highlighted;
    }

    fn matches(&self, token: &Token<&str>) -> bool {
        self.token.as_ref().is_some_and(|old| {
            old.kind == token.kind && old.range == token.range && old.query == token.query
        })
    }
}

pub fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let Some(thread) = s
        .selected
        .filter(|_| s.connected() && s.open_picker.is_none() && s.state.is_some())
    else {
        return clear(s);
    };
    let Some(token) = Token::at(&s.draft) else {
        return clear(s);
    };
    if !s.completion.matches(&token) {
        let Token { kind, range, query } = token;
        s.completion.reset();
        s.completion.token = Some(Token {
            kind,
            range,
            query: query.into(),
        });
        if kind == Kind::File {
            s.completion.due = Some(clock + FILE_DELAY.as_secs_f64());
            tx.wake_after(FILE_DELAY);
        }
        return true;
    }
    if s.completion.due.is_some_and(|due| clock >= due) {
        s.completion.due = None;
        let request = s.completion.request;
        let query = token.query.to_owned();
        s.send(
            Command::SearchFiles {
                thread,
                request,
                query,
            },
            tx,
        );
    }
    false
}

fn clear(s: &mut Desktop) -> bool {
    if s.completion.token.is_none() {
        return false;
    }
    s.completion.reset();
    true
}

fn commands<'a>(s: &'a Desktop, query: &'a str) -> impl Iterator<Item = &'a SlashCommand> {
    s.state
        .iter()
        .flat_map(|state| &state.commands)
        .filter(move |command| {
            command
                .name
                .get(..query.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(query))
        })
}

fn files(s: &Desktop) -> &[String] {
    s.completion
        .files
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map_or(&[], Vec::as_slice)
}

/// Attached completion drawer tucked flush behind the composer's top edge when
/// typing a leading `/` command or `@` file reference.
pub(super) fn drawer(s: &Desktop, t: &Theme) -> Option<El> {
    let token = s
        .completion
        .token
        .as_ref()
        .filter(|_| !s.completion.dismissed && s.draft.focused)?;
    let (heading, rows, empty_message) = match token.kind {
        Kind::Slash => {
            if s.state
                .as_ref()
                .is_none_or(|state| state.commands.is_empty())
            {
                return None;
            }
            let matched: Vec<_> = commands(s, &token.query).collect();
            let active = s
                .completion
                .highlighted
                .min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .into_iter()
                .enumerate()
                .map(|(index, cmd)| slash_row(cmd, index, index == active, t))
                .collect();
            ("Commands", rows, "No matching commands")
        }
        Kind::File => {
            let matched = files(s);
            let active = s
                .completion
                .highlighted
                .min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, path)| file_row(path, index, index == active, t))
                .collect();
            let empty = match &s.completion.files {
                None => "Searching files…",
                Some(Err(_)) => "File search failed",
                Some(Ok(_)) => "No matching files",
            };
            ("Files", rows, empty)
        }
    };
    let header = row()
        .key("completion-header")
        .w_full()
        .h(HEADER_HEIGHT)
        .px(S12)
        .center_y()
        .child(text(heading).font(t.small_medium).color(t.faint))
        .child(rune::spacer())
        .child(
            text("↑↓  ↵  Esc")
                .font(t.mono_micro)
                .color(t.faint)
                .a11y_hidden(),
        );
    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label(heading)
        .w_full()
        .max_h(DRAWER_MAX_HEIGHT)
        .min_h(0.0)
        .padding(S4)
        .instant_changes()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_edge_fade(S16);
    if rows.is_empty() {
        list = list.child(
            text(empty_message)
                .role("status")
                .font(t.small)
                .color(t.faint)
                .px(S8)
                .py(S8),
        );
    } else {
        list = list.children(rows);
    }
    Some(
        ui::tucked_deck(POPUP, 25, t.overlay.c.layer_01, t)
            .role("group")
            .label(heading)
            .transition(motion::tucked_menu())
            .child(header)
            .child(ui::rule(t))
            .child(list),
    )
}

fn slash_row(command: &SlashCommand, index: usize, highlighted: bool, t: &Theme) -> El {
    let id = command.name.clone();
    let label = format!("/{}", command.name);
    let mut item = row()
        .key(format!("slash-{}", command.name))
        .role("option")
        .label(&label)
        .description(&command.description)
        .focus_selected(highlighted)
        .preserve_focus_on_press(true)
        .w_full()
        .h(S32)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if highlighted {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .on_hover(move |s: &mut Desktop| s.completion.highlighted = index)
        .on_press_tx(move |s: &mut Desktop, tx| choose(s, &id, tx))
        .child(
            text(&label)
                .font(t.mono)
                .color(if highlighted { t.accent_text } else { t.text })
                .a11y_hidden(),
        )
        .child(
            text(&command.description)
                .font(t.ui)
                .color(t.muted)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        )
        .children(command.hint.as_deref().map(|hint| {
            text(hint)
                .font(t.mono_small)
                .color(t.faint)
                .max_w(HINT_MAX_WIDTH)
                .truncate()
                .a11y_hidden()
        }));
    if highlighted {
        item = item.focus_ring_for("draft", HAIRLINE, -HAIRLINE, t.accent);
    }
    item
}

fn file_row(path: &str, index: usize, highlighted: bool, t: &Theme) -> El {
    let chosen = path.to_owned();
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) if !name.is_empty() => (Some(dir), name),
        _ => (None, path),
    };
    let mut item = row()
        .key(format!("file-{path}"))
        .role("option")
        .label(path)
        .focus_selected(highlighted)
        .preserve_focus_on_press(true)
        .w_full()
        .h(S32)
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .fill(if highlighted {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .on_hover(move |s: &mut Desktop| s.completion.highlighted = index)
        .on_press_tx(move |s: &mut Desktop, tx| choose(s, &chosen, tx))
        .child(
            icon(&ui::FILE, ICON_SMALL + 1.0)
                .color(if highlighted { t.accent_text } else { t.muted })
                .a11y_hidden(),
        )
        .child(
            text(name)
                .font(t.ui_medium)
                .color(t.text)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .children(dir.map(|dir| {
            text(dir)
                .font(t.mono_small)
                .color(t.faint)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_middle()
                .a11y_hidden()
        }));
    if highlighted {
        item = item.focus_ring_for("draft", HAIRLINE, -HAIRLINE, t.accent);
    }
    item
}

pub(super) fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.completion.dismissed || ev.mods.cmd || ev.mods.alt || ev.mods.ctrl || ev.mods.shift {
        return false;
    }
    let Some(token) = Token::at(&s.draft).filter(|token| s.completion.matches(token)) else {
        return false;
    };
    let count = match token.kind {
        Kind::Slash => commands(s, token.query).count(),
        Kind::File => files(s).len(),
    };
    match ev.key {
        Key::Escape => {
            s.completion.dismissed = true;
            s.completion.due = None;
        }
        Key::Down | Key::Up if count > 0 => {
            let index = s.completion.highlighted.min(count - 1);
            s.completion.highlighted = if ev.key == Key::Down {
                (index + 1) % count
            } else if index == 0 {
                count - 1
            } else {
                index - 1
            };
        }
        Key::Enter | Key::Tab if count > 0 => {
            let index = s.completion.highlighted.min(count - 1);
            let choice = match token.kind {
                Kind::Slash => commands(s, token.query)
                    .nth(index)
                    .map(|cmd| cmd.name.clone()),
                Kind::File => files(s).get(index).cloned(),
            };
            if let Some(choice) = choice {
                choose(s, &choice, tx);
            }
        }
        _ => return false,
    }
    true
}

fn choose(s: &mut Desktop, id: &str, tx: &mut Tx) {
    let Some(token) = Token::at(&s.draft).filter(|token| s.completion.matches(token)) else {
        return;
    };
    let (kind, Range { start, end }) = (token.kind, token.range);
    let mut insertion = match kind {
        Kind::Slash => format!("/{id}"),
        Kind::File if id.contains([' ', '\t', '\n', '"', '\\']) => {
            format!("@\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Kind::File => format!("@{id}"),
    };
    if !s.draft.text[end..].starts_with(char::is_whitespace) {
        insertion.push(' ');
    }
    s.draft.set_selection(start, end);
    s.draft.insert(&insertion);
    if !insertion.ends_with(' ')
        && let Some(separator) = s.draft.text[s.draft.focus..].chars().next()
    {
        s.draft.set_caret(s.draft.focus + separator.len_utf8());
    }
    s.completion.reset();
    tx.focus("draft", false);
}
