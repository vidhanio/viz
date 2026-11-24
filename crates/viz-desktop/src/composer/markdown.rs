//! Markdown structure and keyboard behavior for the live composer draft.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rune::{Color, Key, KeyEvent, Span, TextEdit};
use rune_kit::syntax::highlight;

use crate::theme::Theme;

#[derive(Default)]
pub struct Decorations {
    pub spans: Vec<Span>,
    pub strikes: Vec<(usize, usize, Option<Color>)>,
}

/// Structure derived from the editor text and atomic reference marks.
#[derive(Default)]
pub struct DraftOutline {
    pub pills: Vec<super::codec::DraftPill>,
    pub char_count: usize,
    marks: Vec<(Range<usize>, String)>,
    decorations: [Decorations; 2],
    fences: Vec<Fence>,
    continuations: std::collections::BTreeMap<usize, LineContinuation>,
}

impl DraftOutline {
    pub fn parse(edit: &mut TextEdit) -> Self {
        let mut out = Self {
            char_count: edit.text.chars().count(),
            marks: edit
                .pills()
                .map(|(range, mark)| (range, mark.to_owned()))
                .collect(),
            ..Self::default()
        };
        out.pills = out
            .marks
            .iter()
            .filter_map(|(range, mark)| {
                Some(super::codec::DraftPill {
                    range: range.clone(),
                    reference: super::codec::PillRef::from_mark(mark)?,
                })
            })
            .collect();
        out.pills.sort_by_key(|pill| pill.range.start);

        let mut masks: Vec<_> = out.marks.iter().map(|(range, _)| range.clone()).collect();
        masks.sort_by_key(|range| range.start);
        let masked = PillMask::new(&mut edit.text, &masks);
        let text = masked.text.as_str();
        let parser = Parser::new_ext(
            text,
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS,
        );
        let themes = [Theme::get(false), Theme::get(true)];
        for (_, def) in parser.reference_definitions().iter() {
            if let Some(close) = text[def.span.clone()].find("]:") {
                let start = def.span.start;
                let end = start + close + 2;
                for (style, theme) in out.decorations.iter_mut().zip(themes) {
                    style.spans.push(
                        Span::new(start, end)
                            .mono()
                            .color(theme.accent_text.alpha(0.75)),
                    );
                    if end < def.span.end {
                        style
                            .spans
                            .push(Span::new(end, def.span.end).mono().color(theme.faint));
                    }
                }
            }
        }
        let mut fence = None;
        let mut body_end = 0;
        for (event, range) in parser.into_offset_iter() {
            if range.is_empty() || text.get(range.clone()).is_none() {
                continue;
            }
            match &event {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) => {
                    fence = Some((range.start, info.clone()));
                    continue;
                }
                Event::Text(_) if fence.is_some() => {
                    body_end = range.end;
                }
                Event::End(TagEnd::CodeBlock) => {
                    if let Some((start, info)) = fence.take() {
                        let parsed = Fence::new(text, start..range.end, body_end);
                        parsed.decorate(text, &info, &mut out.decorations, themes);
                        out.fences.push(parsed);
                        body_end = 0;
                    }
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    let (mut offset, _) = line_bounds(text, range.start);
                    for line in text[offset..range.end].split('\n') {
                        if let Some(cont) = continuation_for_line(line) {
                            out.continuations.insert(offset, cont);
                        }
                        offset += line.len() + 1;
                    }
                }
                Event::Start(Tag::Item) => {
                    let (start, end) = line_bounds(text, range.start);
                    if let Some(cont) = continuation_for_line(&text[start..end]) {
                        out.continuations.insert(start, cont);
                    }
                }
                _ => {}
            }
            if overlaps_pill(range.start, range.end, &masks) && is_inline_event(&event) {
                continue;
            }
            for (style, theme) in out.decorations.iter_mut().zip(themes) {
                decorate_event(text, &event, range.clone(), theme, style);
            }
        }
        out
    }

    pub fn marks_match(&self, edit: &TextEdit) -> bool {
        edit.pills().eq(self
            .marks
            .iter()
            .map(|(range, mark)| (range.clone(), mark.as_str())))
    }

    pub fn decorations(&self, theme: &Theme) -> &Decorations {
        &self.decorations[usize::from(theme.k.dark)]
    }

    pub fn in_fence(&self, caret: usize) -> bool {
        self.fence_at(caret).is_some()
    }

    fn fence_at(&self, caret: usize) -> Option<&Fence> {
        let next = self
            .fences
            .partition_point(|fence| fence.editable.end <= caret);
        self.fences
            .get(next)
            .filter(|fence| fence.editable.contains(&caret))
    }
}

fn decorate_event(
    text: &str,
    event: &Event<'_>,
    range: Range<usize>,
    t: &Theme,
    out: &mut Decorations,
) {
    match event {
        Event::Start(Tag::Heading { level, .. }) => style_heading(text, range, *level, t, out),
        Event::Start(Tag::BlockQuote(_)) => style_blockquote(text, range, t, out),
        Event::Start(Tag::Item) => style_list_item(text, range, t, out),
        Event::TaskListMarker(checked) => style_task_marker(text, range, *checked, t, out),
        Event::Rule => out
            .spans
            .push(Span::new(range.start, range.end).color(t.faint)),
        Event::Start(Tag::Emphasis) => {
            style_delimited_inline(text, range, 1, t, out, |s, e| Span::new(s, e).italic());
        }
        Event::Start(Tag::Strong) => {
            style_delimited_inline(text, range, 2, t, out, |s, e| Span::new(s, e).weight(680.0));
        }
        Event::Start(Tag::Strikethrough) => style_strikethrough(text, range, t, out),
        Event::Code(_) => style_inline_code(text, range, t, out),
        Event::Start(Tag::Link { dest_url, .. }) => style_link(text, range, dest_url, t, out),
        Event::Start(Tag::Image { .. }) => style_image(text, range, t, out),
        Event::Start(Tag::CodeBlock(CodeBlockKind::Indented)) => out.spans.push(
            Span::new(range.start, range.end)
                .mono()
                .color(t.text_secondary),
        ),
        _ => {}
    }
}

/// Masks pill labels in place while preserving their source offsets. Only
/// the labels are copied, and the guard restores them before returning.
struct PillMask<'a> {
    text: &'a mut String,
    labels: Vec<(Range<usize>, String)>,
}

impl<'a> PillMask<'a> {
    fn new(text: &'a mut String, ranges: &[Range<usize>]) -> Self {
        let mut masked = Self {
            text,
            labels: Vec::with_capacity(ranges.len()),
        };
        let mut placeholder = String::new();
        for range in ranges {
            let Some(label) = masked.text.get(range.clone()) else {
                continue;
            };
            placeholder.clear();
            placeholder.extend(label.bytes().map(|byte| match byte {
                b'\n' | b'\r' => char::from(byte),
                _ => 'x',
            }));
            masked.labels.push((range.clone(), label.to_owned()));
            masked.text.replace_range(range.clone(), &placeholder);
        }
        masked
    }
}

impl Drop for PillMask<'_> {
    fn drop(&mut self) {
        for (range, label) in self.labels.drain(..).rev() {
            self.text.replace_range(range, &label);
        }
    }
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
    let next = pills.partition_point(|pill| pill.end <= start);
    pills.get(next).is_some_and(|pill| pill.start < end)
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

fn style_link(text: &str, range: Range<usize>, dest_url: &str, t: &Theme, out: &mut Decorations) {
    let slice = &text[range.clone()];
    let is_file = dest_url.starts_with("file://");
    let syntax_ink = t.faint;
    if slice.starts_with('[')
        && let Some(close_bracket) = find_closing_bracket(slice)
    {
        let label_start = range.start + 1;
        let label_end = range.start + close_bracket;
        if is_file {
            let pill_bg = flat_pill_bg(t);
            out.spans
                .push(Span::new(range.start, range.end).bg(pill_bg, 2.0));
        }
        out.spans
            .push(Span::new(range.start, label_start).color(syntax_ink));
        if label_start < label_end {
            let span = if is_file {
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

fn style_image(text: &str, range: Range<usize>, t: &Theme, out: &mut Decorations) {
    let slice = &text[range.clone()];
    let syntax_ink = t.faint;
    let chip_bg = flat_pill_bg(t);
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

fn flat_pill_bg(t: &Theme) -> Color {
    t.text.alpha(if t.k.dark { 0.08 } else { 0.06 })
}

struct Fence {
    editable: Range<usize>,
    opening: Range<usize>,
    body: Range<usize>,
    closing: Range<usize>,
    prefix: String,
}

impl Fence {
    fn new(text: &str, block: Range<usize>, body_end: usize) -> Self {
        let (line_start, opening_end) = line_bounds(text, block.start);
        let body_start = (opening_end + 1).min(block.end);
        let marker = text.as_bytes()[block.start];
        let marker_end = text[..block.end].trim_end_matches(' ').len();
        let marker_len = text[..marker_end]
            .bytes()
            .rev()
            .take_while(|&b| b == marker)
            .count();
        let marker_start = marker_end - marker_len;
        let closed = opening_end < marker_start && marker_len >= 3 && body_end <= marker_start;
        let (closing_line, _) = line_bounds(text, marker_start);
        let end = if closed {
            closing_line
        } else {
            block.end + usize::from(block.end == text.len())
        };
        let prefix = text[line_start..block.start]
            .chars()
            .map(|ch| if ch == '>' || ch == '\t' { ch } else { ' ' })
            .collect();
        Self {
            editable: line_start..end,
            opening: block.start..opening_end.min(block.end),
            body: body_start..if closed { closing_line } else { block.end },
            closing: if closed {
                marker_start..block.end
            } else {
                block.end..block.end
            },
            prefix,
        }
    }

    fn decorate(&self, text: &str, info: &str, styles: &mut [Decorations; 2], themes: [&Theme; 2]) {
        let opening = &text[self.opening.clone()];
        let ticks = opening
            .bytes()
            .take_while(|&byte| byte == opening.as_bytes()[0])
            .count();
        let label = opening[ticks..].trim_start();
        let label_start = self.opening.end - label.len();
        let label_end = label_start + label.split_whitespace().next().map_or(0, str::len);
        let syntax = highlight(info.split_whitespace().next(), &text[self.body.clone()]);
        for (style, theme) in styles.iter_mut().zip(themes) {
            for range in [
                self.opening.start..label_start,
                label_end..self.opening.end,
                self.closing.clone(),
            ] {
                if !range.is_empty() {
                    style
                        .spans
                        .push(Span::new(range.start, range.end).mono().color(theme.faint));
                }
            }
            if label_start < label_end {
                style.spans.push(
                    Span::new(label_start, label_end)
                        .mono()
                        .color(theme.accent_text.alpha(0.85)),
                );
            }
            if !self.body.is_empty() {
                style
                    .spans
                    .push(Span::new(self.body.start, self.body.end).mono());
                style.spans.extend(
                    theme
                        .syntax
                        .spans(&syntax, self.body.start)
                        .into_iter()
                        .map(Span::mono),
                );
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
pub fn handle_key(edit: &mut TextEdit, outline: &DraftOutline, ev: &KeyEvent) -> bool {
    if edit.preedit().is_some() {
        return false;
    }
    let primary = ev.mods.cmd || ev.mods.ctrl;

    if ev.key == Key::Tab && !primary && !ev.mods.alt {
        return handle_tab(edit, outline, ev.mods.shift);
    }

    if ev.key == Key::Enter && !primary {
        return handle_enter(edit, outline, ev.mods.shift || ev.mods.alt);
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

fn handle_enter(edit: &mut TextEdit, outline: &DraftOutline, newline_mod: bool) -> bool {
    let caret = edit.focus;
    if let Some(fence) = outline.fence_at(caret) {
        let (start, end) = line_bounds(&edit.text, caret);
        let line = &edit.text[start..end];
        let marker_prefix = parse_quote_prefix(line).unwrap_or(0);
        let whitespace = line[marker_prefix..]
            .bytes()
            .take_while(|&b| b == b' ' || b == b'\t')
            .count();
        let prefix = &line[..marker_prefix + whitespace];
        let indent = if prefix.len() >= fence.prefix.len() {
            prefix
        } else {
            &fence.prefix
        };
        let mut insertion = String::with_capacity(1 + indent.len());
        insertion.push('\n');
        insertion.push_str(indent);
        edit.insert(&insertion);
        return true;
    }
    if !newline_mod || edit.anchor != edit.focus {
        return false;
    }
    let (line_start, line_end) = line_bounds(&edit.text, caret);
    let line = &edit.text[line_start..line_end];

    if let Some(cont) = outline.continuations.get(&line_start) {
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

fn handle_tab(edit: &mut TextEdit, outline: &DraftOutline, outdent: bool) -> bool {
    let caret = edit.focus;
    let in_fence = outline.in_fence(caret);
    let (line_start, line_end) = line_bounds(&edit.text, caret);
    let line = &edit.text[line_start..line_end];
    let is_list = outline
        .continuations
        .get(&line_start)
        .is_some_and(|cont| cont.list);

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
    list: bool,
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
                list: true,
                prefix_len: quote_len + extra_len,
                next_prefix: next,
            });
        }
        return Some(LineContinuation {
            list: false,
            prefix_len: quote_len,
            next_prefix: line[..quote_len].to_owned(),
        });
    }
    let list = parse_list_prefix(line)?;
    let (prefix_len, next_prefix) = list_continuation(line, &list);
    Some(LineContinuation {
        list: true,
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
