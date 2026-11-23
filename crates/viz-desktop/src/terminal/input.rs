//! Adapt Rune events to Ghostty's mode-aware encoders.

use ghostty_vt::{Terminal, focus, key, mouse, paste, terminal::Mode};
use rune::{Key, KeyEvent, Mods};

use super::{CELL, PAD};

type Result<T> = std::result::Result<T, ghostty_vt::Error>;

pub(super) struct Encoder {
    key: key::Encoder,
    key_event: key::Event,
    mouse: mouse::Encoder,
    mouse_event: mouse::Event,
}

impl Encoder {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            key: key::Encoder::new()?,
            key_event: key::Event::new()?,
            mouse: mouse::Encoder::new()?,
            mouse_event: mouse::Event::new()?,
        })
    }

    pub(super) fn key(
        &mut self,
        terminal: &Terminal,
        event: &KeyEvent,
        base: Option<char>,
    ) -> Result<Option<Vec<u8>>> {
        let physical = key_code(&event.key, base);
        let value = match &event.key {
            Key::Text(text) => Some(text.as_str()),
            _ => None,
        };
        let base = base.or_else(|| match event.key {
            Key::Command(c) => Some(c),
            _ => value.and_then(|text| text.chars().next()),
        });
        if physical == key::Key::Unidentified && value.is_none() {
            return Ok(None);
        }
        self.key_event
            .set_action(key::Action::Press)
            .set_key(physical)
            .set_mods(modifiers(event.mods))
            .set_unshifted_codepoint(base.unwrap_or('\0').to_ascii_lowercase())
            .set_utf8(value);
        let mut bytes = Vec::new();
        self.key
            .set_options_from_terminal(terminal)
            .encode_to_vec(&self.key_event, &mut bytes)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(super) fn mouse(
        &mut self,
        terminal: &Terminal,
        action: mouse::Action,
        button: mouse::Button,
        position: [f32; 2],
        mods: Mods,
    ) -> Result<Vec<u8>> {
        let width = u32::from(CELL.width);
        let height = u32::from(CELL.height);
        self.mouse
            .set_options_from_terminal(terminal)
            .set_any_button_pressed(action == mouse::Action::Motion)
            .set_size(mouse::EncoderSize {
                screen_width: u32::from(terminal.cols()?) * width,
                screen_height: u32::from(terminal.rows()?) * height,
                cell_width: width,
                cell_height: height,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
            });
        self.mouse_event
            .set_action(action)
            .set_button(Some(button))
            .set_mods(modifiers(mods))
            .set_position(mouse::Position {
                x: position[0] - PAD,
                y: position[1] - PAD,
            });
        let mut bytes = Vec::new();
        self.mouse.encode_to_vec(&self.mouse_event, &mut bytes)?;
        Ok(bytes)
    }
}

pub(super) fn paste(terminal: &Terminal, text: &str) -> Result<Vec<u8>> {
    let mut input = text.as_bytes().to_vec();
    let mut bytes = vec![0; input.len() + 12];
    let len = paste::encode(
        &mut input,
        terminal.mode(Mode::BRACKETED_PASTE)?,
        &mut bytes,
    )?;
    bytes.truncate(len);
    Ok(bytes)
}

pub(super) fn focus(terminal: &Terminal, event: focus::Event) -> Result<Option<Vec<u8>>> {
    if !terminal.mode(Mode::FOCUS_EVENT)? {
        return Ok(None);
    }
    let mut bytes = [0; 3];
    let len = event.encode(&mut bytes)?;
    Ok(Some(bytes[..len].to_vec()))
}

fn modifiers(mods: Mods) -> key::Mods {
    let mut value = key::Mods::empty();
    value.set(key::Mods::ALT, mods.alt);
    value.set(
        key::Mods::CTRL,
        mods.ctrl || (!cfg!(target_os = "macos") && mods.cmd),
    );
    value.set(key::Mods::SHIFT, mods.shift);
    value.set(key::Mods::SUPER, cfg!(target_os = "macos") && mods.cmd);
    value
}

#[expect(
    clippy::too_many_lines,
    reason = "the explicit Rune to Ghostty key table"
)]
fn key_code(event: &Key, base: Option<char>) -> key::Key {
    use key::Key as K;
    const LETTERS: [K; 26] = [
        K::A,
        K::B,
        K::C,
        K::D,
        K::E,
        K::F,
        K::G,
        K::H,
        K::I,
        K::J,
        K::K,
        K::L,
        K::M,
        K::N,
        K::O,
        K::P,
        K::Q,
        K::R,
        K::S,
        K::T,
        K::U,
        K::V,
        K::W,
        K::X,
        K::Y,
        K::Z,
    ];
    const DIGITS: [K; 10] = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    const FUNCTIONS: [K; 24] = [
        K::F1,
        K::F2,
        K::F3,
        K::F4,
        K::F5,
        K::F6,
        K::F7,
        K::F8,
        K::F9,
        K::F10,
        K::F11,
        K::F12,
        K::F13,
        K::F14,
        K::F15,
        K::F16,
        K::F17,
        K::F18,
        K::F19,
        K::F20,
        K::F21,
        K::F22,
        K::F23,
        K::F24,
    ];
    match event {
        Key::Enter => K::Enter,
        Key::Backspace => K::Backspace,
        Key::Delete => K::Delete,
        Key::Left => K::ArrowLeft,
        Key::Right => K::ArrowRight,
        Key::Up => K::ArrowUp,
        Key::Down => K::ArrowDown,
        Key::Home => K::Home,
        Key::End => K::End,
        Key::PageUp => K::PageUp,
        Key::PageDown => K::PageDown,
        Key::Insert => K::Insert,
        Key::Escape => K::Escape,
        Key::Tab => K::Tab,
        Key::Function(index) => FUNCTIONS
            .get(usize::from(index.saturating_sub(1)))
            .copied()
            .unwrap_or(K::Unidentified),
        Key::Command(_) | Key::Text(_) => {
            let base = base.or_else(|| match event {
                Key::Command(c) => Some(*c),
                Key::Text(text) => text.chars().next(),
                _ => None,
            });
            match base.map(|c| c.to_ascii_lowercase()) {
                Some(c @ 'a'..='z') => LETTERS[c as usize - 'a' as usize],
                Some(c @ '0'..='9') => DIGITS[c as usize - '0' as usize],
                Some(' ') => K::Space,
                Some('[') => K::BracketLeft,
                Some(']') => K::BracketRight,
                Some('\\') => K::Backslash,
                Some('-') => K::Minus,
                Some('=') => K::Equal,
                Some('/') => K::Slash,
                Some('.') => K::Period,
                Some(',') => K::Comma,
                Some(';') => K::Semicolon,
                Some('\'') => K::Quote,
                _ => K::Unidentified,
            }
        }
        _ => K::Unidentified,
    }
}
