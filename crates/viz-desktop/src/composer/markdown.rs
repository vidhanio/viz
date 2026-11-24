//! Live Markdown editor helpers over a plain [`rune::TextEdit`]: WYSIWYG-style
//! per-frame [`Span`] and strikethrough decorations for headings, bold, italic,
//! strikethrough, inline code, links, blockquotes, lists, task items, and
//! language-highlighted fenced code blocks, plus invisible keyboard ergonomics
//! (fence-aware Enter, list/quote continuation, list/code indentation, and
//! Markdown delimiter shortcuts). No buttons or hidden rich-text state.

use std::ops::Range;

use rune::{Color, Key, KeyEvent, Span, TextEdit};
use rune_kit::syntax::highlight;

use crate::theme::Theme;

/// Per-frame visual decorations computed directly from plain Markdown text.
pub struct Decorations {
    pub spans: Vec<Span>,
    pub strikes: Vec<(usize, usize, Option<Color>)>,
}

/// Computes live WYSIWYG-style Markdown decorations for `edit`, skipping any
/// atomic `Mark::Pill` byte ranges so mention chips stay untouched.
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

    let mut ctx = DecorateCtx {
        text,
        pills: &pills,
        t,
        out: &mut out,
    };
    ctx.scan_document();
    out
}

struct DecorateCtx<'a> {
    text: &'a str,
    pills: &'a [Range<usize>],
    t: &'a Theme,
    out: &'a mut Decorations,
}

struct OpenFence<'a> {
    ch: u8,
    len: usize,
    lang: Option<&'a str>,
    body_start: usize,
}

impl<'a> DecorateCtx<'a> {
    fn scan_document(&mut self) {
        let mut fence: Option<OpenFence<'a>> = None;
        let mut pos = 0usize;
        for raw_line in self.text.split('\n') {
            let line_start = pos;
            let line_end = pos + raw_line.len();
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let content_end = line_start + line.len();
            pos = (line_end + 1).min(self.text.len());

            if let Some(open) = &fence {
                if is_closing_fence(line, open.ch, open.len) {
                    let body_end = line_start.saturating_sub(1).max(open.body_start);
                    self.highlight_code_body(open.lang, open.body_start, body_end);
                    self.dim_mono(line_start, content_end);
                    fence = None;
                } else if line_start < content_end {
                    self.out
                        .spans
                        .push(Span::new(line_start, content_end).mono());
                }
                continue;
            }

            if let Some(parsed) = parse_opening_fence(line) {
                self.style_opening_fence(line_start, content_end, &parsed);
                fence = Some(OpenFence {
                    ch: parsed.ch,
                    len: parsed.len,
                    lang: parsed.lang,
                    body_start: pos,
                });
                continue;
            }

            self.style_block_line(line_start, content_end);
        }

        if let Some(open) = fence
            && open.body_start < self.text.len()
        {
            self.highlight_code_body(open.lang, open.body_start, self.text.len());
        }
    }

    fn style_opening_fence(&mut self, line_start: usize, line_end: usize, fence: &FenceOpen<'a>) {
        let ticks_end = line_start + fence.ticks_end;
        self.dim_mono(line_start, ticks_end);
        if let Some((ls, le)) = fence.lang_range {
            self.out.spans.push(
                Span::new(line_start + ls, line_start + le)
                    .mono()
                    .color(self.t.accent_text.alpha(0.85)),
            );
            if line_start + le < line_end {
                self.dim_mono(line_start + le, line_end);
            }
        } else if ticks_end < line_end {
            self.dim_mono(ticks_end, line_end);
        }
    }

    fn highlight_code_body(&mut self, lang: Option<&str>, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let Some(code) = self.text.get(start..end) else {
            return;
        };
        let ranges = highlight(lang, code);
        for span in self.t.syntax.spans(&ranges, start) {
            self.out.spans.push(span.mono());
        }
    }

    fn dim_mono(&mut self, start: usize, end: usize) {
        if start < end {
            self.out
                .spans
                .push(Span::new(start, end).mono().color(self.t.faint));
        }
    }

    fn style_block_line(&mut self, line_start: usize, line_end: usize) {
        if line_start >= line_end {
            return;
        }
        let mut cursor = line_start;
        let line = &self.text[line_start..line_end];

        if is_thematic_break(line) {
            self.out
                .spans
                .push(Span::new(line_start, line_end).color(self.t.faint));
            return;
        }

        if let Some(quote_len) = parse_quote_prefix(line) {
            self.style_quote_markers(line_start, line_start + quote_len);
            cursor = line_start + quote_len;
            if cursor < line_end {
                self.out
                    .spans
                    .push(Span::new(cursor, line_end).color(self.t.muted));
            }
        }

        let rest = &self.text[cursor..line_end];
        if let Some((hashes, text_off)) = parse_heading_prefix(rest) {
            self.out
                .spans
                .push(Span::new(cursor, cursor + text_off).color(self.t.faint));
            let body_start = cursor + text_off;
            if body_start < line_end {
                let bump = match hashes {
                    1 => 3.0,
                    2 => 1.5,
                    3 => 0.5,
                    _ => 0.0,
                };
                let mut span = Span::new(body_start, line_end).weight(680.0);
                if bump > 0.0 {
                    span = span.size(self.t.body.size + bump);
                }
                self.out.spans.push(span);
                self.scan_inline(body_start, line_end);
            }
            return;
        }

        if let Some(list) = parse_list_prefix(rest) {
            let marker_start = cursor + list.marker_start;
            let marker_end = cursor + list.marker_end;
            self.out.spans.push(
                Span::new(marker_start, marker_end)
                    .bold()
                    .color(self.t.accent_text.alpha(0.85)),
            );
            cursor += list.content_start;
            if let Some((checked, box_end)) = parse_task_box(&self.text[cursor..line_end]) {
                let color = if checked {
                    self.t.accent_text
                } else {
                    self.t.muted
                };
                self.out
                    .spans
                    .push(Span::new(cursor, cursor + 3).mono().color(color));
                cursor += box_end;
                if checked && cursor < line_end {
                    self.out
                        .spans
                        .push(Span::new(cursor, line_end).color(self.t.muted));
                    self.out
                        .strikes
                        .push((cursor, line_end, Some(self.t.muted)));
                }
            }
        }

        self.scan_inline(cursor, line_end);
    }

    fn style_quote_markers(&mut self, start: usize, end: usize) {
        let color = self.t.accent_text.alpha(0.8);
        for (offset, byte) in self.text[start..end].bytes().enumerate() {
            if byte == b'>' {
                let at = start + offset;
                self.out
                    .spans
                    .push(Span::new(at, at + 1).bold().color(color));
            }
        }
    }

    fn scan_inline(&mut self, start: usize, end: usize) {
        let bytes = self.text.as_bytes();
        let mut i = start;
        while i < end {
            if let Some(pill_end) = self.pill_ending_at_or_after(i) {
                i = pill_end.min(end);
                continue;
            }
            match bytes[i] {
                b'\\' if i + 1 < end && bytes[i + 1].is_ascii_punctuation() => {
                    self.out.spans.push(Span::new(i, i + 1).color(self.t.faint));
                    i += 2;
                }
                b'`' => {
                    if let Some(next) = self.try_inline_code(i, end) {
                        i = next;
                    } else {
                        i += 1;
                    }
                }
                b'[' => {
                    if let Some(next) = self.try_link(i, end) {
                        i = next;
                    } else {
                        i += 1;
                    }
                }
                b'~' if i + 1 < end && bytes[i + 1] == b'~' => {
                    if let Some(next) = self.try_strike(i, end) {
                        i = next;
                    } else {
                        i += 2;
                    }
                }
                b'*' | b'_' => {
                    if let Some(next) = self.try_emphasis(i, end) {
                        i = next;
                    } else {
                        i += 1;
                    }
                }
                _ => {
                    let ch = self.text[i..end].chars().next().map_or(1, char::len_utf8);
                    i += ch;
                }
            }
        }
    }

    fn try_inline_code(&mut self, start: usize, end: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let mut ticks = 0usize;
        while start + ticks < end && bytes[start + ticks] == b'`' {
            ticks += 1;
        }
        let body_start = start + ticks;
        let close = find_matching_ticks(bytes, body_start, end, ticks)?;
        let close_end = close + ticks;
        if close <= body_start || self.overlaps_pill(start, close_end) {
            return None;
        }
        let bg = self.t.chip_fill;
        self.out
            .spans
            .push(Span::new(start, close_end).mono().bg(bg, 2.0));
        self.out
            .spans
            .push(Span::new(start, body_start).color(self.t.faint));
        self.out
            .spans
            .push(Span::new(close, close_end).color(self.t.faint));
        Some(close_end)
    }

    fn try_link(&mut self, start: usize, end: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let close_bracket = find_byte_unescaped(bytes, start + 1, end, b']')?;
        if close_bracket + 1 >= end || bytes[close_bracket + 1] != b'(' {
            return None;
        }
        let url_start = close_bracket + 2;
        let close_paren = find_closing_paren(bytes, url_start, end)?;
        let total_end = close_paren + 1;
        if self.overlaps_pill(start, total_end) {
            return None;
        }
        let label_start = start + 1;
        self.out
            .spans
            .push(Span::new(start, label_start).color(self.t.faint));
        if label_start < close_bracket {
            self.out.spans.push(
                Span::new(label_start, close_bracket)
                    .color(self.t.accent_text)
                    .underline(self.t.accent_text.alpha(0.55)),
            );
            self.scan_inline(label_start, close_bracket);
        }
        self.out
            .spans
            .push(Span::new(close_bracket, total_end).color(self.t.faint));
        Some(total_end)
    }

    fn try_strike(&mut self, start: usize, end: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let inner_start = start + 2;
        if inner_start >= end || bytes[inner_start].is_ascii_whitespace() {
            return None;
        }
        let close = find_double_delim(bytes, inner_start, end, b'~')?;
        if close <= inner_start || bytes[close - 1].is_ascii_whitespace() {
            return None;
        }
        let total_end = close + 2;
        if self.overlaps_pill(start, total_end) {
            return None;
        }
        self.out
            .spans
            .push(Span::new(start, inner_start).color(self.t.faint));
        self.out
            .spans
            .push(Span::new(inner_start, close).color(self.t.muted));
        self.out
            .strikes
            .push((inner_start, close, Some(self.t.muted)));
        self.out
            .spans
            .push(Span::new(close, total_end).color(self.t.faint));
        self.scan_inline(inner_start, close);
        Some(total_end)
    }

    fn try_emphasis(&mut self, start: usize, end: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let ch = bytes[start];
        if ch == b'_' && is_word_byte(start.checked_sub(1).and_then(|idx| bytes.get(idx))) {
            return None;
        }
        let mut run = 0usize;
        while start + run < end && bytes[start + run] == ch && run < 3 {
            run += 1;
        }
        for len in (1..=run).rev() {
            let inner_start = start + len;
            if inner_start >= end || bytes[inner_start].is_ascii_whitespace() {
                continue;
            }
            let Some(close) = find_emphasis_close(bytes, inner_start, end, ch, len) else {
                continue;
            };
            let total_end = close + len;
            if self.overlaps_pill(start, total_end) {
                continue;
            }
            self.out
                .spans
                .push(Span::new(start, inner_start).color(self.t.faint));
            let inner = match len {
                3 => Span::new(inner_start, close).weight(680.0).italic(),
                2 => Span::new(inner_start, close).weight(680.0),
                _ => Span::new(inner_start, close).italic(),
            };
            self.out.spans.push(inner);
            self.out
                .spans
                .push(Span::new(close, total_end).color(self.t.faint));
            self.scan_inline(inner_start, close);
            return Some(total_end);
        }
        None
    }

    fn pill_ending_at_or_after(&self, byte: usize) -> Option<usize> {
        self.pills
            .iter()
            .find(|r| r.start <= byte && byte < r.end)
            .map(|r| r.end)
    }

    fn overlaps_pill(&self, start: usize, end: usize) -> bool {
        self.pills.iter().any(|r| r.start < end && start < r.end)
    }
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

/// Returns `true` when `caret` lies on an opening fence line or inside an open
/// or closed fenced code block (excluding the closing fence line itself).
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

struct FenceOpen<'a> {
    ch: u8,
    len: usize,
    ticks_end: usize,
    lang: Option<&'a str>,
    lang_range: Option<(usize, usize)>,
}

fn parse_opening_fence(line: &str) -> Option<FenceOpen<'_>> {
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
    let (lang, lang_range) = if lang_len > 0 {
        let le = lang_start + lang_len;
        (Some(&line[lang_start..le]), Some((lang_start, le)))
    } else {
        (None, None)
    };
    Some(FenceOpen {
        ch,
        len,
        ticks_end,
        lang,
        lang_range,
    })
}

fn is_closing_fence(line: &str, ch: u8, open_len: usize) -> bool {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    let count = bytes[indent..].iter().take_while(|&&b| b == ch).count();
    count >= open_len && bytes[indent + count..].iter().all(u8::is_ascii_whitespace)
}

fn is_thematic_break(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(&marker) = trimmed.as_bytes().first() else {
        return false;
    };
    if !matches!(marker, b'-' | b'*' | b'_') {
        return false;
    }
    let mut count = 0usize;
    for b in trimmed.bytes() {
        if b == marker {
            count += 1;
        } else if b != b' ' && b != b'\t' {
            return false;
        }
    }
    count >= 3
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

fn parse_heading_prefix(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let indent = bytes.iter().take(3).take_while(|&&b| b == b' ').count();
    let hashes = bytes[indent..].iter().take_while(|&&b| b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = indent + hashes;
    match bytes.get(after) {
        Some(b' ' | b'\t') => Some((hashes, after + 1)),
        None => Some((hashes, after)),
        _ => None,
    }
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

const fn find_matching_ticks(
    bytes: &[u8],
    mut i: usize,
    end: usize,
    ticks: usize,
) -> Option<usize> {
    while i < end {
        if bytes[i] == b'`' {
            let start = i;
            while i < end && bytes[i] == b'`' {
                i += 1;
            }
            if i - start == ticks {
                return Some(start);
            }
        } else {
            i += 1;
        }
    }
    None
}

const fn find_byte_unescaped(bytes: &[u8], mut i: usize, end: usize, needle: u8) -> Option<usize> {
    while i < end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

const fn find_closing_paren(bytes: &[u8], mut i: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'\\' => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                i += 1;
            }
            b' ' | b'\t' if depth == 0 => return None,
            _ => i += 1,
        }
    }
    None
}

const fn find_double_delim(bytes: &[u8], mut i: usize, end: usize, ch: u8) -> Option<usize> {
    while i + 1 < end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let mut ticks = 0usize;
            while i + ticks < end && bytes[i + ticks] == b'`' {
                ticks += 1;
            }
            if let Some(close) = find_matching_ticks(bytes, i + ticks, end, ticks) {
                i = close + ticks;
                continue;
            }
            i += ticks;
            continue;
        }
        if bytes[i] == ch && bytes[i + 1] == ch {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_emphasis_close(
    bytes: &[u8],
    mut i: usize,
    end: usize,
    ch: u8,
    len: usize,
) -> Option<usize> {
    while i + len <= end {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let mut ticks = 0usize;
            while i + ticks < end && bytes[i + ticks] == b'`' {
                ticks += 1;
            }
            if let Some(close) = find_matching_ticks(bytes, i + ticks, end, ticks) {
                i = close + ticks;
                continue;
            }
            i += ticks;
            continue;
        }
        if bytes[i] == ch {
            let run_start = i;
            while i < end && bytes[i] == ch {
                i += 1;
            }
            let run_len = i - run_start;
            let matches_len = run_len == len || (len == 1 && run_len == 3);
            if matches_len && run_start > 0 && !bytes[run_start - 1].is_ascii_whitespace() {
                let close_at = run_start + run_len - len;
                if ch == b'_' && is_word_byte(bytes.get(close_at + len)) {
                    continue;
                }
                return Some(close_at);
            }
            continue;
        }
        i += 1;
    }
    None
}

const fn is_word_byte(byte: Option<&u8>) -> bool {
    matches!(byte, Some(b) if b.is_ascii_alphanumeric())
}
