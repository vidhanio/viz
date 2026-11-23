//! ANSI CSI / SGR escape parsing onto flat base16 [`Theme`] text spans.

use rune::{Color, Span};

use crate::theme::Theme;

/// Strips ANSI CSI escapes and maps standard SGR colour codes onto base16
/// [`Theme`] tokens for crisp terminal output rendering.
pub fn parse_ansi(input: &str, t: &Theme) -> (String, Vec<Span>) {
    if !input.contains('\x1b') {
        return (input.to_owned(), Vec::new());
    }
    let mut plain = String::with_capacity(input.len());
    let mut spans = Vec::new();
    let mut active_color: Option<Color> = None;
    let mut active_bold = false;
    let mut active_italic = false;
    let mut span_start = 0usize;

    let flush = |spans: &mut Vec<Span>,
                 start: usize,
                 end: usize,
                 color: Option<Color>,
                 bold: bool,
                 italic: bool| {
        if end > start && (color.is_some() || bold || italic) {
            spans.push(Span {
                color,
                weight: bold.then_some(600.0),
                italic,
                ..Span::new(start, end)
            });
        }
    };

    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            flush(
                &mut spans,
                span_start,
                plain.len(),
                active_color,
                active_bold,
                active_italic,
            );
            i += 2;
            let param_start = i;
            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                let final_byte = bytes[i];
                let params = &input[param_start..i];
                i += 1;
                if final_byte == b'm' {
                    apply_sgr(
                        params,
                        t,
                        &mut active_color,
                        &mut active_bold,
                        &mut active_italic,
                    );
                }
            }
            span_start = plain.len();
        } else {
            let ch = input[i..].chars().next().unwrap_or(' ');
            plain.push(ch);
            i += ch.len_utf8();
        }
    }
    flush(
        &mut spans,
        span_start,
        plain.len(),
        active_color,
        active_bold,
        active_italic,
    );
    (plain, spans)
}

fn apply_sgr(
    params: &str,
    t: &Theme,
    color: &mut Option<Color>,
    bold: &mut bool,
    italic: &mut bool,
) {
    if params.is_empty() {
        *color = None;
        *bold = false;
        *italic = false;
        return;
    }
    for code in params.split(';') {
        match code.parse::<u16>().unwrap_or(0) {
            0 => {
                *color = None;
                *bold = false;
                *italic = false;
            }
            1 => *bold = true,
            2 => *color = Some(t.muted),
            3 => *italic = true,
            22 => *bold = false,
            23 => *italic = false,
            30 | 90 => *color = Some(t.faint),
            31 | 91 => *color = Some(t.danger),
            32 | 92 => *color = Some(t.success),
            33 | 93 => *color = Some(t.warning),
            34 | 94 => *color = Some(t.accent_text),
            35 | 95 => *color = Some(t.delegated),
            36 | 96 => *color = Some(t.info),
            37 | 97 => *color = Some(t.text),
            39 => *color = None,
            _ => {}
        }
    }
}
