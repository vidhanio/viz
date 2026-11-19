use std::{ops::Range, time::Duration};

use rune::{El, Key, KeyEvent, TextEdit, Tx, icon, text};
use rune_kit::PopupItem;
use viz_client::protocol::SlashCommand;

use crate::{
    app::Desktop,
    backend::Command,
    theme::{ICON_SMALL, Theme},
    ui,
};

const FILE_DELAY: Duration = Duration::from_millis(110);
const POPUP: &str = "composer-completion";

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

    fn matches(&self, token: &Token<&str>) -> bool {
        self.token.as_ref().is_some_and(|old| {
            old.kind == token.kind && old.range == token.range && old.query == token.query
        })
    }
}

pub fn poll(s: &mut Desktop, clock: f64, tx: &mut Tx) -> bool {
    let Some(conversation) = s.selected.filter(|_| {
        s.connected()
            && s.open_picker.is_none()
            && s.conversation
                .as_ref()
                .is_some_and(|state| !state.read_only)
    }) else {
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
                conversation,
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
    s.conversation
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

pub(super) fn popup(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let Some(token) = s
        .completion
        .token
        .as_ref()
        .filter(|_| !s.completion.dismissed && s.draft.focused)
    else {
        return;
    };
    let popup = ui::popup_list::<Desktop>(POPUP, "composer-card")
        .width(ui::COMPLETION_WIDTH)
        .active(Some(s.completion.highlighted))
        .on_active(|s, i| s.completion.highlighted = i)
        .on_choose(choose);
    let popup = match token.kind {
        Kind::Slash
            if s.conversation
                .as_ref()
                .is_some_and(|state| !state.commands.is_empty()) =>
        {
            popup.label("Commands").empty_message("No commands").items(
                commands(s, &token.query).map(|command| {
                    let mut item = PopupItem::new(&command.name, format!("/{}", command.name))
                        .description(&command.description);
                    if let Some(hint) = &command.hint {
                        item = item.trailing(
                            text(hint)
                                .font(t.mono_small)
                                .color(t.faint)
                                .max_w(ui::COMPLETION_WIDTH / 3.0)
                                .truncate(),
                        );
                    }
                    item
                }),
            )
        }
        Kind::Slash => return,
        Kind::File => popup
            .label("Files")
            .empty_message(match &s.completion.files {
                None => "Searching files",
                Some(Err(_)) => "File search failed",
                Some(Ok(_)) => "No files",
            })
            .items(files(s).iter().map(|path| {
                PopupItem::new(path, path).leading(icon(&ui::SEARCH, ICON_SMALL).color(t.muted))
            })),
    };
    popup.build(&t.k, layer);
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
