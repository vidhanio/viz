//! View and key adapter for the composer's tucked `/` and `@` completion
//! drawer.

use rune::{Color, El, Key, KeyEvent, ScrollSpec, Tx, col, icon, row, text};
use viz_client::protocol::SlashCommand;

use crate::{
    app::Desktop,
    model::{CompletionKind, completion_token_at, matching_commands, matching_files},
    motion,
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, S12, S16, S32, Theme},
    ui,
    update::{ComposerMsg, Msg, NavKey},
};

const POPUP: &str = "composer-completion";
const LIST: &str = "completion-list";
/// Maximum scrollable height of the completion drawer before scrolling.
const DRAWER_MAX_HEIGHT: f32 = 248.0;
/// Micro-header height at the top of the completion drawer.
const HEADER_HEIGHT: f32 = 26.0;
/// Maximum width of a slash command's argument hint.
const HINT_MAX_WIDTH: f32 = 140.0;

/// Attached completion drawer tucked flush behind the composer's top edge when
/// typing a leading `/` command or `@` file reference.
pub(super) fn drawer(s: &Desktop, t: &Theme) -> Option<El> {
    let m = &s.model;
    let completion = &m.composer.completion;
    let token = completion
        .token
        .as_ref()
        .filter(|_| !completion.dismissed && s.widgets.draft.focused)?;
    let (heading, rows, empty_message) = match token.kind {
        CompletionKind::Slash => {
            if m.thread_state()
                .is_none_or(|state| state.commands.is_empty())
            {
                return None;
            }
            let matched = matching_commands(m, &token.query);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .into_iter()
                .enumerate()
                .map(|(index, cmd)| slash_row(cmd, index, index == active, t))
                .collect();
            ("Commands", rows, "No matching commands")
        }
        CompletionKind::File => {
            let matched = matching_files(m);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, path)| file_row(path, index, index == active, t))
                .collect();
            let empty = match &completion.files {
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
        .on_hover(move |s: &mut Desktop| {
            s.dispatch_local(Msg::Composer(ComposerMsg::CompletionHover(index)));
        })
        .on_press_tx(move |s: &mut Desktop, tx| {
            let cursor = s.draft_cursor();
            s.dispatch(
                Msg::Composer(ComposerMsg::ChooseCompletion {
                    id: id.clone(),
                    cursor,
                }),
                tx,
            );
        })
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
        .on_hover(move |s: &mut Desktop| {
            s.dispatch_local(Msg::Composer(ComposerMsg::CompletionHover(index)));
        })
        .on_press_tx(move |s: &mut Desktop, tx| {
            let cursor = s.draft_cursor();
            s.dispatch(
                Msg::Composer(ComposerMsg::ChooseCompletion {
                    id: chosen.clone(),
                    cursor,
                }),
                tx,
            );
        })
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
    if s.model.composer.completion.dismissed
        || ev.mods.cmd
        || ev.mods.alt
        || ev.mods.ctrl
        || ev.mods.shift
    {
        return false;
    }
    let cursor = s.draft_cursor();
    let Some(token) = completion_token_at(&s.widgets.draft.text, &cursor)
        .filter(|token| s.model.composer.completion.matches(token))
    else {
        return false;
    };
    let count = match token.kind {
        CompletionKind::Slash => matching_commands(&s.model, &token.query).len(),
        CompletionKind::File => matching_files(&s.model).len(),
    };
    let key = match ev.key {
        Key::Escape => NavKey::Escape,
        Key::Down if count > 0 => NavKey::Down,
        Key::Up if count > 0 => NavKey::Up,
        Key::Enter if count > 0 => NavKey::Enter,
        Key::Tab if count > 0 => NavKey::Tab,
        _ => return false,
    };
    s.dispatch(
        Msg::Composer(ComposerMsg::CompletionKey { key, cursor }),
        tx,
    );
    true
}
