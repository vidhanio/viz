//! Live Markdown editor helpers over a plain [`rune::TextEdit`], parsed with
//! `pulldown-cmark` (`into_offset_iter`): WYSIWYG-style per-frame [`Span`] and
//! strikethrough decorations for headings, bold, italic, strikethrough, inline
//! code, links, embedded attachment images/references, blockquotes, lists, task
//! items, and language-highlighted fenced code blocks, plus invisible keyboard
//! ergonomics (fence-aware Enter, list/quote continuation, indentation, and
//! Markdown delimiter shortcuts).

use std::{borrow::Cow, ops::Range};

use pulldown_cmark::{BrokenLink, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use rune::{Color, Key, KeyEvent, Span, TextEdit};
use rune_kit::syntax::highlight;

use crate::theme::Theme;

/// Per-frame visual decorations computed directly from plain Markdown text.
pub struct Decorations {
    pub spans: Vec<Span>,
    pub strikes: Vec<(usize, usize, Option<Color>)>,
}

/// Computes live WYSIWYG-style Markdown decorations for `edit` using
/// `pulldown-cmark`, masking atomic `Mark::Pill` byte ranges so mention chips
/// never interact with Markdown delimiters.
#[must_use]
pub fn decorate(edit: &TextEdit, t: &Theme) -> Decorations {
    let text = edit.text.as_str();
    let mut out = Decorations {
        spans: Vec::new(),
        strikes: Vec::new(),
    };
    if text.is_empty() {
        return out;
    }

    let mut pills: Vec<Range<usize>> = edit.pills().map(|(r, _)| r).collect();
    pills.sort_by_key(|r| r.start);
    let masked = mask_pills(text, &pills);
    let src: &str = &masked;

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut broken = |link: BrokenLink<'_>| {
        let reference = link.reference.trim();
        if is_attachment_ref(reference) {
            Some((format!("viz-ref://{reference}").into(), "".into()))
        } else {
            None
        }
    };

    let mut code_ranges: Vec<Range<usize>> = Vec::new();
    let caret = edit.focus;
    let parser = Parser::new_with_broken_link_callback(src, opts, Some(&mut broken));

    for (event, range) in parser.into_offset_iter() {
        if range.start >= range.end || range.end > text.len() {
            continue;
        }
        if overlaps_pill(range.start, range.end, &pills) && is_inline_event(&event) {
            continue;
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                style_heading(text, range, level, t, &mut out);
            }
            Event::Start(Tag::BlockQuote(_)) => {
                style_blockquote(text, range, t, &mut out);
            }
            Event::Start(Tag::Item) => {
                style_list_item(text, range, t, &mut out);
            }
            Event::TaskListMarker(checked) => {
                style_task_marker(text, range, checked, t, &mut out);
            }
            Event::Rule => {
                out.spans
                    .push(Span::new(range.start, range.end).color(t.faint));
            }
            Event::Start(Tag::Emphasis) => {
                style_delimited_inline(text, range, 1, t, &mut out, |s, e| {
                    Span::new(s, e).italic()
                });
            }
            Event::Start(Tag::Strong) => {
                style_delimited_inline(text, range, 2, t, &mut out, |s, e| {
                    Span::new(s, e).weight(680.0)
                });
            }
            Event::Start(Tag::Strikethrough) => {
                style_strikethrough(text, range, t, &mut out);
            }
            Event::Code(_) => {
                style_inline_code(text, range, t, &mut out);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                style_link(text, range, &dest_url, caret, t, &mut out);
            }
            Event::Start(Tag::Image { .. }) => {
                style_image(text, range, caret, t, &mut out);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                code_ranges.push(range.clone());
                style_code_block(text, range, kind, t, &mut out);
            }
            _ => {}
        }
    }

    style_reference_definitions(text, &code_ranges, t, &mut out);
    out
}

/// Replaces bytes covered by atomic `Mark::Pill` ranges with ASCII spaces of
/// equal byte length so `pulldown-cmark` preserves exact UTF-8 byte offsets
/// without treating underscores or punctuation inside `@file` pills as
/// Markdown delimiters.
fn mask_pills<'a>(text: &'a str, pills: &[Range<usize>]) -> Cow<'a, str> {
    if pills.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut bytes = text.as_bytes().to_vec();
    for pill in pills {
        let start = pill.start.min(bytes.len());
        let end = pill.end.min(bytes.len());
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    Cow::Owned(String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned()))
}

const fn is_inline_event(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Start(
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
        ) | Event::Code(_)
    )
}

fn overlaps_pill(start: usize, end: usize, pills: &[Range<usize>]) -> bool {
    pills.iter().any(|r| r.start < end && start < r.end)
}

/// Whether `reference` looks like an inline attachment reference (`img-1`,
/// `file-2`, `att-3`) that resolves against the message's attachments.
#[must_use]
pub fn is_attachment_ref(reference: &str) -> bool {
    let lower = reference.to_ascii_lowercase();
    for prefix in ["img-", "file-", "att-"] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

fn style_heading(
    text: &str,
    range: Range<usize>,
    level: HeadingLevel,
    t: &Theme,
    out: &mut Decorations,
) {
    let slice = &text[range.clone()];
    let bytes = slice.as_bytes();
    let indent = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    let hashes = bytes[indent..].iter().take_while(|&&b| b == b'#').count();
    if hashes == 0 {
        return;
    }
    let prefix_end =
        (indent + hashes + usize::from(matches!(bytes.get(indent + hashes), Some(b' ' | b'\t'))))
            .min(slice.len());
    out.spans
        .push(Span::new(range.start, range.start + prefix_end).color(t.faint));
    let body_start = range.start + prefix_end;
    let body_end = range.start + slice.trim_end_matches(['\r', '\n']).len();
    if body_start < body_end {
        let bump = match level {
            HeadingLevel::H1 => 3.0,
            HeadingLevel::H2 => 1.5,
            HeadingLevel::H3 => 0.5,
            HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => 0.0,
        };
        let mut span = Span::new(body_start, body_end).weight(680.0);
        if bump > 0.0 {
            span = span.size(t.body.size + bump);
        }
        out.spans.push(span);
    }
}

fn style_blockquote(text: &str, range: Range<usize>, t: &Theme, out: &mut Decorations) {
    let mut pos = range.start;
    let accent = t.accent_text.alpha(0.8);
    for raw_line in text[range.clone()].split('\n') {
        let line_start = pos;
        let line_end = pos + raw_line.len();
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let content_end = line_start + line.len();
        pos = (line_end + 1).min(range.end);

        if let Some(qlen) = parse_quote_prefix(line) {
            for (off, byte) in line[..qlen].bytes().enumerate() {
                if byte == b'>' {
                    let at = line_start + off;
                    out.spans.push(Span::new(at, at + 1).bold().color(accent));
                }
            }
            if line_start + qlen < content_end {
                out.spans
                    .push(Span::new(line_start + qlen, content_end).color(t.muted));
            }
        }
    }
}

fn style_list_item(text: &str, range: Range<usize>, t: &Theme, out: &mut Decorations) {
    let first_line_end = text[range.clone()]
        .find('\n')
        .map_or(range.end, |off| range.start + off);
    let first_line = text[range.start..first_line_end].trim_end_matches('\r');
    let line_start = text[..range.start].rfind('\n').map_or(0, |idx| idx + 1);
    let full_first = &text[line_start..first_line_end];
    let stripped_offset = parse_quote_prefix(full_first).unwrap_or(0);
    let probe = &full_first[stripped_offset..];
    if let Some(list) = parse_list_prefix(probe) {
        let base = line_start + stripped_offset;
        out.spans.push(
            Span::new(base + list.marker_start, base + list.marker_end)
                .bold()
                .color(t.accent_text.alpha(0.85)),
        );
    } else if let Some(list) = parse_list_prefix(first_line) {
        out.spans.push(
            Span::new(
                range.start + list.marker_start,
                range.start + list.marker_end,
            )
            .bold()
            .color(t.accent_text.alpha(0.85)),
        );
    }
}

fn style_task_marker(
    text: &str,
    range: Range<usize>,
    checked: bool,
    t: &Theme,
    out: &mut Decorations,
) {
    let color = if checked { t.accent_text } else { t.muted };
    out.spans
        .push(Span::new(range.start, range.end).mono().color(color));
    if checked {
        let line_end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |off| range.end + off);
        let body_start = range.end
            + text[range.end..line_end]
                .bytes()
                .take_while(|&b| b == b' ' || b == b'\t')
                .count();
        let body_end = text[body_start..line_end].trim_end_matches('\r').len() + body_start;
        if body_start < body_end {
            out.spans
                .push(Span::new(body_start, body_end).color(t.muted));
            out.strikes.push((body_start, body_end, Some(t.muted)));
        }
    }
}

fn style_delimited_inline(
    text: &str,
    range: Range<usize>,
    delim_len: usize,
    t: &Theme,
    out: &mut Decorations,
    make_inner: impl FnOnce(usize, usize) -> Span,
) {
    if range.len() <= delim_len * 2 {
        return;
    }
    let slice = &text[range.clone()];
    let bytes = slice.as_bytes();
    let ch = bytes[0];
    if !matches!(ch, b'*' | b'_') {
        return;
    }
    let open_ok = bytes
        .get(..delim_len)
        .is_some_and(|s| s.iter().all(|&b| b == ch));
    let close_ok = bytes
        .get(bytes.len() - delim_len..)
        .is_some_and(|s| s.iter().all(|&b| b == ch));
    if !open_ok || !close_ok {
        return;
    }
    let inner_start = range.start + delim_len;
    let inner_end = range.end - delim_len;
    out.spans
        .push(Span::new(range.start, inner_start).color(t.faint));
    out.spans.push(make_inner(inner_start, inner_end));
    out.spans
        .push(Span::new(inner_end, range.end).color(t.faint));
}

fn style_strikethrough(text: &str, range: Range<usize>, t: &Theme, out: &mut Decorations) {
    let bytes = text[range.clone()].as_bytes();
    let open_tildes = bytes.iter().take_while(|&&b| b == b'~').count().min(2);
    let close_tildes = bytes
        .iter()
        .rev()
        .take_while(|&&b| b == b'~')
        .count()
        .min(2);
    if open_tildes == 0 || close_tildes == 0 || range.len() <= open_tildes + close_tildes {
        return;
    }
    let inner_start = range.start + open_tildes;
    let inner_end = range.end - close_tildes;
    out.spans
        .push(Span::new(range.start, inner_start).color(t.faint));
    out.spans
        .push(Span::new(inner_start, inner_end).color(t.muted));
    out.strikes.push((inner_start, inner_end, Some(t.muted)));
    out.spans
        .push(Span::new(inner_end, range.end).color(t.faint));
}

fn style_inline_code(text: &str, range: Range<usize>, t: &Theme, out: &mut Decorations) {
    let bytes = text[range.clone()].as_bytes();
    let open_ticks = bytes.iter().take_while(|&&b| b == b'`').count();
    let close_ticks = bytes.iter().rev().take_while(|&&b| b == b'`').count();
    out.spans.push(
        Span::new(range.start, range.end)
            .mono()
            .bg(t.chip_fill, 2.0),
    );
    if open_ticks > 0 && close_ticks > 0 && range.len() > open_ticks + close_ticks {
        out.spans
            .push(Span::new(range.start, range.start + open_ticks).color(t.faint));
        out.spans
            .push(Span::new(range.end - close_ticks, range.end).color(t.faint));
    }
}

fn style_link(
    text: &str,
    range: Range<usize>,
    dest_url: &str,
    caret: usize,
    t: &Theme,
    out: &mut Decorations,
) {
    let slice = &text[range.clone()];
    let is_file = dest_url.starts_with("file://");
    let is_ref = dest_url.starts_with("viz-ref://");
    let editing = caret >= range.start && caret <= range.end;
    let syntax_ink = if editing { t.muted } else { t.faint };
    if slice.starts_with('[')
        && let Some(close_bracket) = find_closing_bracket(slice)
    {
        let label_start = range.start + 1;
        let label_end = range.start + close_bracket;
        if is_file || is_ref {
            let pill_bg = flat_pill_bg(editing, t);
            out.spans
                .push(Span::new(range.start, range.end).bg(pill_bg, 2.0));
        }
        out.spans
            .push(Span::new(range.start, label_start).color(syntax_ink));
        if label_start < label_end {
            let span = if is_file || is_ref {
                Span::new(label_start, label_end)
                    .weight(500.0)
                    .color(t.text_strong)
            } else {
                Span::new(label_start, label_end)
                    .color(t.accent_text)
                    .underline(t.accent_text.alpha(0.55))
            };
            out.spans.push(span);
        }
        out.spans
            .push(Span::new(label_end, range.end).color(syntax_ink));
        return;
    }
    out.spans.push(
        Span::new(range.start, range.end)
            .color(if is_file {
                t.text_strong
            } else {
                t.accent_text
            })
            .underline(if is_file {
                t.text.alpha(0.35)
            } else {
                t.accent_text.alpha(0.55)
            }),
    );
}

fn style_image(text: &str, range: Range<usize>, caret: usize, t: &Theme, out: &mut Decorations) {
    let slice = &text[range.clone()];
    let editing = caret >= range.start && caret <= range.end;
    let syntax_ink = if editing { t.muted } else { t.faint };
    let chip_bg = flat_pill_bg(editing, t);
    out.spans
        .push(Span::new(range.start, range.end).bg(chip_bg, 2.0));
    if slice.starts_with("![")
        && let Some(close_bracket) = find_closing_bracket(&slice[1..])
    {
        let close_pos = close_bracket + 1;
        let alt_start = range.start + 2;
        let alt_end = range.start + close_pos;
        out.spans
            .push(Span::new(range.start, alt_start).color(syntax_ink));
        if alt_start < alt_end {
            out.spans.push(
                Span::new(alt_start, alt_end)
                    .weight(500.0)
                    .color(t.text_strong),
            );
        }
        out.spans
            .push(Span::new(alt_end, range.end).mono().color(syntax_ink));
    } else {
        out.spans
            .push(Span::new(range.start, range.end).color(t.text_strong));
    }
}

fn flat_pill_bg(editing: bool, t: &Theme) -> rune::Color {
    let alpha = match (editing, t.k.dark) {
        (true, true) => 0.14,
        (true, false) => 0.11,
        (false, true) => 0.08,
        (false, false) => 0.06,
    };
    t.text.alpha(alpha)
}

fn style_code_block(
    text: &str,
    range: Range<usize>,
    kind: CodeBlockKind<'_>,
    t: &Theme,
    out: &mut Decorations,
) {
    let CodeBlockKind::Fenced(info) = kind else {
        out.spans.push(
            Span::new(range.start, range.end)
                .mono()
                .color(t.text_secondary),
        );
        return;
    };
    let slice = &text[range.clone()];
    let first_nl = slice.find('\n');
    let first_line_end = first_nl.map_or(range.end, |off| range.start + off);
    let first_line = text[range.start..first_line_end].trim_end_matches('\r');
    let open = parse_opening_fence(first_line);

    if let Some(fence) = &open {
        let ticks_end = range.start + fence.ticks_end;
        out.spans
            .push(Span::new(range.start, ticks_end).mono().color(t.faint));
        let content_end = range.start + first_line.len();
        if let Some((ls, le)) = fence.lang_range {
            out.spans.push(
                Span::new(range.start + ls, range.start + le)
                    .mono()
                    .color(t.accent_text.alpha(0.85)),
            );
            if range.start + le < content_end {
                out.spans.push(
                    Span::new(range.start + le, content_end)
                        .mono()
                        .color(t.faint),
                );
            }
        } else if ticks_end < content_end {
            out.spans
                .push(Span::new(ticks_end, content_end).mono().color(t.faint));
        }
    }

    let Some(nl_off) = first_nl else {
        return;
    };
    let body_start = range.start + nl_off + 1;
    if body_start >= range.end {
        return;
    }

    let trimmed_end = text[body_start..range.end]
        .trim_end_matches(['\r', '\n'])
        .len()
        + body_start;
    let last_line_start = text[body_start..trimmed_end]
        .rfind('\n')
        .map_or(body_start, |off| body_start + off + 1);
    let last_line = text[last_line_start..trimmed_end].trim_end_matches('\r');

    let body_end = if let Some(fence) = &open
        && is_closing_fence(last_line, fence.ch, fence.len)
    {
        out.spans.push(
            Span::new(last_line_start, last_line_start + last_line.len())
                .mono()
                .color(t.faint),
        );
        last_line_start.saturating_sub(1).max(body_start)
    } else {
        range.end
    };

    if body_start < body_end {
        out.spans.push(Span::new(body_start, body_end).mono());
        let lang = info
            .split_whitespace()
            .next()
            .filter(|word| !word.is_empty());
        if let Some(code) = text.get(body_start..body_end) {
            let ranges = highlight(lang, code);
            for span in t.syntax.spans(&ranges, body_start) {
                out.spans.push(span.mono());
            }
        }
    }
}

fn style_reference_definitions(
    text: &str,
    code_ranges: &[Range<usize>],
    t: &Theme,
    out: &mut Decorations,
) {
    let mut pos = 0usize;
    for raw_line in text.split('\n') {
        let line_start = pos;
        let line_end = pos + raw_line.len();
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        pos = (line_end + 1).min(text.len());
        if code_ranges
            .iter()
            .any(|r| r.start <= line_start && line_start < r.end)
        {
            continue;
        }
        let indent = line
            .as_bytes()
            .iter()
            .take(3)
            .take_while(|&&b| b == b' ')
            .count();
        let trimmed = &line[indent..];
        if trimmed.starts_with('[')
            && let Some(close) = trimmed.find("]:")
            && close > 1
        {
            let label_start = line_start + indent;
            let colon_end = label_start + close + 2;
            out.spans.push(
                Span::new(label_start, colon_end)
                    .mono()
                    .color(t.accent_text.alpha(0.75)),
            );
            let content_end = line_start + line.len();
            if colon_end < content_end {
                out.spans
                    .push(Span::new(colon_end, content_end).mono().color(t.faint));
            }
        }
    }
}

const fn find_closing_bracket(slice: &str) -> Option<usize> {
    let bytes = slice.as_bytes();
    let mut i = 1usize;
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'[' => {
                depth += 1;
                i += 1;
            }
            b']' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Handles Markdown keyboard ergonomics on the composer [`TextEdit`]:
/// fence-aware Enter (so Return inside a ```` ```lang ```` block inserts a
/// newline instead of submitting), list/blockquote continuation on Shift+Enter,
/// Tab/Shift+Tab indentation in lists and code fences, selection wrapping with
/// `*`/`~`/`` ` ``, and Markdown delimiter shortcuts (`Cmd/Ctrl+I`,
/// `Shift+Cmd/Ctrl+B`, `Shift+Cmd/Ctrl+X`, `Cmd/Ctrl+E`).
pub fn handle_key(edit: &mut TextEdit, ev: &KeyEvent) -> bool {
    if edit.preedit().is_some() {
        return false;
    }
    let primary = ev.mods.cmd || ev.mods.ctrl;

    if ev.key == Key::Tab && !primary && !ev.mods.alt {
        return handle_tab(edit, ev.mods.shift);
    }

    if ev.key == Key::Enter && !primary {
        return handle_enter(edit, ev.mods.shift || ev.mods.alt);
    }

    if let Key::Text(ch) = &ev.key {
        if primary && !ev.mods.alt {
            return handle_shortcut(edit, ch, ev.mods.shift);
        }
        if !primary && !ev.mods.alt && edit.anchor != edit.focus {
            let wrap = match ch.as_str() {
                "`" => Some("`"),
                "*" => Some("*"),
                "~" => Some("~~"),
                _ => None,
            };
            if let Some(delim) = wrap {
                wrap_selection(edit, delim);
                return true;
            }
        }
    }

    false
}

fn handle_shortcut(edit: &mut TextEdit, ch: &str, shift: bool) -> bool {
    let lower = ch.to_ascii_lowercase();
    let delim = if shift {
        match lower.as_str() {
            "b" => Some("**"),
            "x" => Some("~~"),
            _ => None,
        }
    } else {
        match lower.as_str() {
            "i" => Some("*"),
            "e" | "`" => Some("`"),
            "b" if edit.anchor != edit.focus => Some("**"),
            _ => None,
        }
    };
    let Some(delim) = delim else {
        return false;
    };
    toggle_wrap(edit, delim);
    true
}

fn handle_enter(edit: &mut TextEdit, newline_mod: bool) -> bool {
    let caret = edit.focus;
    if !newline_mod {
        if edit.anchor == edit.focus && is_in_code_fence(&edit.text, caret) {
            let indent = current_line_indent(&edit.text, caret);
            let mut insertion = String::with_capacity(1 + indent.len());
            insertion.push('\n');
            insertion.push_str(indent);
            edit.insert(&insertion);
            return true;
        }
        return false;
    }

    if edit.anchor != edit.focus || is_in_code_fence(&edit.text, caret) {
        return false;
    }
    let (line_start, line_end) = line_bounds(&edit.text, caret);
    let line = &edit.text[line_start..line_end];

    if let Some(cont) = continuation_for_line(line) {
        let rest_trimmed = line[cont.prefix_len..].trim();
        if rest_trimmed.is_empty() && caret >= line_start + cont.prefix_len {
            edit.undo_group(|e| {
                e.set_selection(line_start, line_end);
                e.insert("");
            });
            return true;
        }
        if caret >= line_start + cont.prefix_len {
            let mut insertion = String::with_capacity(1 + cont.next_prefix.len());
            insertion.push('\n');
            insertion.push_str(&cont.next_prefix);
            edit.insert(&insertion);
            return true;
        }
    }
    false
}

fn handle_tab(edit: &mut TextEdit, outdent: bool) -> bool {
    let caret = edit.focus;
    let in_fence = is_in_code_fence(&edit.text, caret);
    let (line_start, line_end) = line_bounds(&edit.text, caret);
    let line = &edit.text[line_start..line_end];
    let is_list = parse_list_prefix(line).is_some();

    if !in_fence && !is_list {
        return false;
    }
    if in_fence && !outdent && edit.anchor == edit.focus {
        edit.insert("  ");
        return true;
    }

    if outdent {
        let removable = line
            .bytes()
            .take(2)
            .take_while(|&b| b == b' ')
            .count()
            .max(usize::from(line.starts_with('\t')));
        if removable == 0 {
            return true;
        }
        let new_focus = caret.saturating_sub(removable).max(line_start);
        edit.undo_group(|e| {
            e.set_selection(line_start, line_start + removable);
            e.insert("");
            e.set_caret(new_focus);
        });
        return true;
    }

    let new_focus = caret + 2;
    edit.undo_group(|e| {
        e.set_selection(line_start, line_start);
        e.insert("  ");
        e.set_caret(new_focus);
    });
    true
}

fn wrap_selection(edit: &mut TextEdit, delim: &str) {
    let (a, b) = (edit.anchor.min(edit.focus), edit.anchor.max(edit.focus));
    let selected = edit.text[a..b].to_owned();
    let dlen = delim.len();
    let mut wrapped = String::with_capacity(selected.len() + dlen * 2);
    wrapped.push_str(delim);
    wrapped.push_str(&selected);
    wrapped.push_str(delim);
    edit.undo_group(|e| {
        e.set_selection(a, b);
        e.insert(&wrapped);
        e.set_selection(a + dlen, a + dlen + selected.len());
    });
}

fn toggle_wrap(edit: &mut TextEdit, delim: &str) {
    let (a, b) = (edit.anchor.min(edit.focus), edit.anchor.max(edit.focus));
    let dlen = delim.len();
    if a == b {
        let mut pair = String::with_capacity(dlen * 2);
        pair.push_str(delim);
        pair.push_str(delim);
        edit.undo_group(|e| {
            e.insert(&pair);
            e.set_caret(a + dlen);
        });
        return;
    }

    let wrapped_outside = a >= dlen
        && b + dlen <= edit.text.len()
        && edit.text.get(a - dlen..a) == Some(delim)
        && edit.text.get(b..b + dlen) == Some(delim);
    if wrapped_outside {
        let inner = edit.text[a..b].to_owned();
        edit.undo_group(|e| {
            e.set_selection(a - dlen, b + dlen);
            e.insert(&inner);
            e.set_selection(a - dlen, a - dlen + inner.len());
        });
        return;
    }

    let selected = &edit.text[a..b];
    if selected.len() >= dlen * 2 && selected.starts_with(delim) && selected.ends_with(delim) {
        let inner = selected[dlen..selected.len() - dlen].to_owned();
        edit.undo_group(|e| {
            e.set_selection(a, b);
            e.insert(&inner);
            e.set_selection(a, a + inner.len());
        });
        return;
    }

    wrap_selection(edit, delim);
}

struct LineContinuation {
    prefix_len: usize,
    next_prefix: String,
}

fn continuation_for_line(line: &str) -> Option<LineContinuation> {
    if let Some(quote_len) = parse_quote_prefix(line) {
        let after = &line[quote_len..];
        if let Some(list) = parse_list_prefix(after) {
            let mut next = line[..quote_len].to_owned();
            let (extra_len, list_next) = list_continuation(after, &list);
            next.push_str(&list_next);
            return Some(LineContinuation {
                prefix_len: quote_len + extra_len,
                next_prefix: next,
            });
        }
        return Some(LineContinuation {
            prefix_len: quote_len,
            next_prefix: line[..quote_len].to_owned(),
        });
    }
    let list = parse_list_prefix(line)?;
    let (prefix_len, next_prefix) = list_continuation(line, &list);
    Some(LineContinuation {
        prefix_len,
        next_prefix,
    })
}

fn list_continuation(line: &str, list: &ListPrefix) -> (usize, String) {
    let indent = &line[..list.marker_start];
    let after = &line[list.content_start..];
    if let Some((_, box_end)) = parse_task_box(after) {
        let marker = &line[list.marker_start..list.marker_end];
        return (
            list.content_start + box_end,
            format!("{indent}{marker} [ ] "),
        );
    }
    if let Some((num, delim)) = list.numbered {
        return (
            list.content_start,
            format!("{indent}{}{delim} ", num.saturating_add(1)),
        );
    }
    let marker = &line[list.marker_start..list.marker_end];
    (list.content_start, format!("{indent}{marker} "))
}

fn line_bounds(text: &str, caret: usize) -> (usize, usize) {
    let caret = caret.min(text.len());
    let start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
    let end = text[caret..]
        .find('\n')
        .map_or(text.len(), |off| caret + off);
    (start, end)
}

fn current_line_indent(text: &str, caret: usize) -> &str {
    let (start, end) = line_bounds(text, caret);
    let line = &text[start..end];
    let spaces = line
        .bytes()
        .take_while(|&b| b == b' ' || b == b'\t')
        .count();
    &line[..spaces]
}

fn is_in_code_fence(text: &str, caret: usize) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    let mut pos = 0usize;
    for raw_line in text.split('\n') {
        let line_start = pos;
        let line_end = pos + raw_line.len();
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let on_line = caret >= line_start && caret <= line_end;
        pos = (line_end + 1).min(text.len());

        if let Some((ch, len)) = fence {
            if is_closing_fence(line, ch, len) {
                if on_line {
                    return false;
                }
                fence = None;
            } else if on_line {
                return true;
            }
            continue;
        }

        if let Some(parsed) = parse_opening_fence(line) {
            if on_line {
                return true;
            }
            fence = Some((parsed.ch, parsed.len));
        } else if on_line {
            return false;
        }
    }
    fence.is_some()
}

struct FenceOpen {
    ch: u8,
    len: usize,
    ticks_end: usize,
    lang_range: Option<(usize, usize)>,
}

fn parse_opening_fence(line: &str) -> Option<FenceOpen> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    let ch = *bytes.get(indent)?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = bytes[indent..].iter().take_while(|&&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let ticks_end = indent + len;
    let info = &line[ticks_end..];
    if ch == b'`' && info.contains('`') {
        return None;
    }
    let ws = info.bytes().take_while(u8::is_ascii_whitespace).count();
    let lang_start = ticks_end + ws;
    let lang_len = line[lang_start..]
        .bytes()
        .take_while(|b| !b.is_ascii_whitespace())
        .count();
    let lang_range = (lang_len > 0).then_some((lang_start, lang_start + lang_len));
    Some(FenceOpen {
        ch,
        len,
        ticks_end,
        lang_range,
    })
}

fn is_closing_fence(line: &str, ch: u8, open_len: usize) -> bool {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    let count = bytes[indent..].iter().take_while(|&&b| b == ch).count();
    count >= open_len && bytes[indent + count..].iter().all(u8::is_ascii_whitespace)
}

fn parse_quote_prefix(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut saw_gt = false;
    while i < bytes.len() {
        let mut spaces = 0usize;
        while i + spaces < bytes.len() && bytes[i + spaces] == b' ' && spaces < 3 {
            spaces += 1;
        }
        if bytes.get(i + spaces) == Some(&b'>') {
            saw_gt = true;
            i += spaces + 1;
            if bytes.get(i) == Some(&b' ') {
                i += 1;
            }
        } else {
            break;
        }
    }
    saw_gt.then_some(i)
}

struct ListPrefix {
    marker_start: usize,
    marker_end: usize,
    content_start: usize,
    numbered: Option<(u64, char)>,
}

fn parse_list_prefix(line: &str) -> Option<ListPrefix> {
    let bytes = line.as_bytes();
    let indent = bytes
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let rest = bytes.get(indent..)?;
    if rest.len() >= 2 && matches!(rest[0], b'-' | b'*' | b'+') && matches!(rest[1], b' ' | b'\t') {
        return Some(ListPrefix {
            marker_start: indent,
            marker_end: indent + 1,
            content_start: indent + 2,
            numbered: None,
        });
    }
    let digits = rest
        .iter()
        .take(9)
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits > 0
        && rest.len() >= digits + 2
        && matches!(rest[digits], b'.' | b')')
        && matches!(rest[digits + 1], b' ' | b'\t')
    {
        let num = line[indent..indent + digits].parse::<u64>().ok()?;
        let delim = rest[digits] as char;
        return Some(ListPrefix {
            marker_start: indent,
            marker_end: indent + digits + 1,
            content_start: indent + digits + 2,
            numbered: Some((num, delim)),
        });
    }
    None
}

const fn parse_task_box(rest: &str) -> Option<(bool, usize)> {
    let bytes = rest.as_bytes();
    if bytes.len() >= 4
        && bytes[0] == b'['
        && matches!(bytes[1], b' ' | b'x' | b'X')
        && bytes[2] == b']'
        && matches!(bytes[3], b' ' | b'\t')
    {
        return Some((bytes[1] != b' ', 4));
    }
    None
}
