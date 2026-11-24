//! Key adapter for the composer's tucked `@`, `/`, `$`, and `#` completion
//! drawers.

use rune::{Key, KeyEvent, Tx};

use crate::{
    app::Desktop,
    model::{completion_count, completion_token_at},
    update::{ComposerMsg, Msg, NavKey},
};

/// Handles `Up`/`Down`, `Ctrl+P`/`Ctrl+N`, `Enter`/`Tab`, and `Escape` while a
/// completion drawer is open.
pub(super) fn on_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.model.composer.completion.dismissed || ev.mods.cmd || ev.mods.alt || ev.mods.shift {
        return false;
    }
    let cursor = s.draft_cursor();
    let Some(token) = completion_token_at(
        &s.widgets.draft.text,
        &cursor,
        &s.widgets.draft_outline.pills,
    )
    .filter(|token| s.model.composer.completion.matches(token)) else {
        return false;
    };
    let count = completion_count(&s.model, &token);
    let key = if ev.mods.ctrl {
        ctrl_nav(&ev.key, count)
    } else {
        match ev.key {
            Key::Escape => Some(NavKey::Escape),
            Key::Down if count > 0 => Some(NavKey::Down),
            Key::Up if count > 0 => Some(NavKey::Up),
            Key::Enter if count > 0 => Some(NavKey::Enter),
            Key::Tab if count > 0 => Some(NavKey::Tab),
            _ => None,
        }
    };
    let Some(key) = key else {
        return false;
    };
    s.dispatch(
        Msg::Composer(ComposerMsg::CompletionKey { key, cursor }),
        tx,
    );
    true
}

fn ctrl_nav(key: &Key, count: usize) -> Option<NavKey> {
    if count == 0 {
        return None;
    }
    match key {
        Key::Text(t) if t.eq_ignore_ascii_case("n") => Some(NavKey::Down),
        Key::Text(t) if t.eq_ignore_ascii_case("p") => Some(NavKey::Up),
        Key::Command('n' | 'N') => Some(NavKey::Down),
        Key::Command('p' | 'P') => Some(NavKey::Up),
        _ => None,
    }
}
