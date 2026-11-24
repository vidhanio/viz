//! Rich-text composer helpers powered by `rune::TextEdit`: Markdown-safe
//! inline marks and paragraph blocks, per-frame syntax highlighting inside
//! `Block::Code`, Markdown auto-triggers, compact formatting toolbar, and
//! lossless Markdown serialization on submit.

use std::fmt::Write as _;

use rune::{Block, El, Key, KeyEvent, Mark, RichStyle, Span, TextEdit, el, row, text};
use rune_kit::{
    Button, ButtonVariant, ControlSize, Divider,
    syntax::{SyntaxTheme, highlight},
};

use crate::{
    app::Desktop,
    theme::{S4, S8, S12, Theme},
};

/// Constructs an empty rich composer [`TextEdit`] constrained to
/// Markdown-compatible inline marks and paragraph blocks.
#[must_use]
pub fn new_edit() -> TextEdit {
    configure(TextEdit::new(""))
}

/// Applies the composer's rich-text mark and block allow-list to `edit`.
#[must_use]
pub fn configure(edit: TextEdit) -> TextEdit {
    edit.rich()
        .allow_marks([
            Mark::Bold,
            Mark::Italic,
            Mark::Strike,
            Mark::Code,
            Mark::Link(String::new()),
        ])
        .allow_blocks([
            Block::Bullet(0),
            Block::Numbered(0),
            Block::Quote,
            Block::Code,
        ])
}

/// Theme-matched [`RichStyle`] for inline code, code blocks, blockquotes,
/// links, and list markers inside the composer field.
#[must_use]
pub const fn style(t: &Theme) -> RichStyle {
    RichStyle {
        link: t.accent_text,
        code_fg: None,
        code_bg: t.chip_fill,
        code_block_bg: t.surface_recessed,
        quote_bar: t.border_strong,
        marker: Some(t.muted),
    }
}

/// Whether `edit` carries any non-pill inline formatting mark or non-paragraph
/// block structure.
#[must_use]
pub fn has_formatting(edit: &TextEdit) -> bool {
    edit.marks()
        .iter()
        .any(|mark| !matches!(mark.mark, Mark::Pill(_)))
        || edit.blocks().iter().any(|block| *block != Block::Paragraph)
}

/// Syntax-highlights contiguous `Block::Code` paragraph runs inside `edit`
/// using `rune_kit::syntax::highlight`.
#[must_use]
pub fn code_block_spans(edit: &TextEdit, theme: &SyntaxTheme) -> Vec<Span> {
    let mut out = Vec::new();
    let paras = edit.paragraphs();
    let mut i = 0;
    while i < paras.len() {
        if paras[i].block == Block::Code {
            let start = paras[i].range.start;
            let mut end = paras[i].range.end;
            while i + 1 < paras.len() && paras[i + 1].block == Block::Code {
                i += 1;
                end = paras[i].range.end;
            }
            if let Some(code) = edit.text.get(start..end) {
                let ranges = highlight(Some("rust"), code);
                out.extend(theme.spans(&ranges, start));
            }
        }
        i += 1;
    }
    out
}

/// Handles composer rich-text shortcuts, list Tab/Shift+Tab indentation, and
/// start-of-line Markdown block triggers (`- `, `* `, `1. `, `> `, ```` ```
/// ````). Returns `true` when the key was consumed.
pub fn handle_key(edit: &mut TextEdit, ev: &KeyEvent) -> bool {
    if edit.preedit().is_some() {
        return false;
    }
    if ev.key == Key::Tab && !ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt {
        return if ev.mods.shift {
            edit.outdent_list()
        } else {
            edit.indent_list()
        };
    }
    if let Key::Text(ch) = &ev.key {
        let primary = ev.mods.cmd || ev.mods.ctrl;
        if primary && !ev.mods.alt {
            let lower = ch.to_ascii_lowercase();
            if ev.mods.shift {
                match lower.as_str() {
                    "b" => {
                        edit.toggle_mark(Mark::Bold);
                        return true;
                    }
                    "x" => {
                        edit.toggle_mark(Mark::Strike);
                        return true;
                    }
                    "c" => {
                        edit.toggle_block(Block::Code);
                        return true;
                    }
                    "7" => {
                        edit.toggle_block(Block::Numbered(0));
                        return true;
                    }
                    "8" => {
                        edit.toggle_block(Block::Bullet(0));
                        return true;
                    }
                    "9" => {
                        edit.toggle_block(Block::Quote);
                        return true;
                    }
                    _ => {}
                }
            } else if lower == "e" || lower == "`" {
                edit.toggle_mark(Mark::Code);
                return true;
            } else if lower == "b" && edit.anchor != edit.focus {
                edit.toggle_mark(Mark::Bold);
                return true;
            }
        }
        if !primary && !ev.mods.alt && ch == " " && trigger_block_on_space(edit) {
            return true;
        }
    }
    ev.key == Key::Enter
        && !ev.mods.ctrl
        && !ev.mods.cmd
        && !ev.mods.alt
        && !ev.mods.shift
        && trigger_code_fence_on_enter(edit)
}

/// Converts leading Markdown block markers (`-`, `*`, `1.`, `>`, ```` ``` ````)
/// into rich [`Block`]s when the user presses Space at the start of a
/// paragraph.
fn trigger_block_on_space(edit: &mut TextEdit) -> bool {
    if edit.anchor != edit.focus || edit.current_block() != Block::Paragraph {
        return false;
    }
    let caret = edit.focus;
    let line_start = edit.text[..caret].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &edit.text[line_start..caret];
    let target = match prefix {
        "-" | "*" => Block::Bullet(0),
        "1." => Block::Numbered(0),
        ">" => Block::Quote,
        "```" => Block::Code,
        _ => return false,
    };
    edit.undo_group(|e| {
        e.set_selection(line_start, caret);
        e.insert("");
        e.set_block(target);
    });
    true
}

/// Converts a bare ```` ``` ```` line into a `Block::Code` block on Enter.
fn trigger_code_fence_on_enter(edit: &mut TextEdit) -> bool {
    if edit.anchor != edit.focus || edit.current_block() != Block::Paragraph {
        return false;
    }
    let caret = edit.focus;
    let line_start = edit.text[..caret].rfind('\n').map_or(0, |idx| idx + 1);
    if &edit.text[line_start..caret] != "```" {
        return false;
    }
    edit.undo_group(|e| {
        e.set_selection(line_start, caret);
        e.insert("");
        e.set_block(Block::Code);
    });
    true
}

/// Compact inline and block formatting strip mounted above the composer field
/// when toggled open or when the draft uses rich formatting.
pub fn toolbar(s: &Desktop, t: &Theme) -> El {
    let edit = &s.widgets.draft;
    let blk = edit.current_block();
    let bar = row()
        .key("composer-format-bar")
        .w_full()
        .px(S12)
        .pt(S8)
        .pb(S4)
        .gap(S4)
        .center_y()
        .child(mark_btn(
            "fmt-bold",
            "B",
            "Bold (Shift+Cmd/Ctrl+B)",
            Mark::Bold,
            edit,
            t,
        ))
        .child(mark_btn(
            "fmt-italic",
            "I",
            "Italic (Cmd/Ctrl+I)",
            Mark::Italic,
            edit,
            t,
        ))
        .child(mark_btn(
            "fmt-strike",
            "S",
            "Strikethrough (Shift+Cmd/Ctrl+X)",
            Mark::Strike,
            edit,
            t,
        ))
        .child(mark_btn(
            "fmt-code",
            "<>",
            "Inline code (Cmd/Ctrl+E)",
            Mark::Code,
            edit,
            t,
        ))
        .child(el().w(S4))
        .child(block_btn(
            "fmt-bullet",
            "• List",
            "Bullet list",
            Block::Bullet(0),
            matches!(blk, Block::Bullet(_)),
            t,
        ))
        .child(block_btn(
            "fmt-numbered",
            "1. List",
            "Numbered list",
            Block::Numbered(0),
            matches!(blk, Block::Numbered(_)),
            t,
        ))
        .child(block_btn(
            "fmt-quote",
            "Quote",
            "Blockquote",
            Block::Quote,
            blk == Block::Quote,
            t,
        ))
        .child(block_btn(
            "fmt-codeblock",
            "Code",
            "Code block",
            Block::Code,
            blk == Block::Code,
            t,
        ));
    rune::col()
        .w_full()
        .child(bar)
        .child(Divider::horizontal().build(&t.k))
}

fn mark_btn(
    key: &'static str,
    label: &'static str,
    tip: &'static str,
    mark: Mark,
    edit: &TextEdit,
    t: &Theme,
) -> El {
    let active = edit.has_mark(&mark);
    fmt_btn(key, label, tip, active, t, move |s| {
        s.widgets.draft.toggle_mark(mark.clone());
        s.sync_draft_text();
    })
}

fn block_btn(
    key: &'static str,
    label: &'static str,
    tip: &'static str,
    block: Block,
    active: bool,
    t: &Theme,
) -> El {
    fmt_btn(key, label, tip, active, t, move |s| {
        s.widgets.draft.toggle_block(block);
        s.sync_draft_text();
    })
}

fn fmt_btn(
    key: &'static str,
    label: &'static str,
    tip: &'static str,
    active: bool,
    t: &Theme,
    on_press: impl Fn(&mut Desktop) + 'static,
) -> El {
    Button::new(label)
        .control_size(ControlSize::Small)
        .variant(if active {
            ButtonVariant::Contrast
        } else {
            ButtonVariant::GhostMuted
        })
        .on_press(on_press)
        .build(&t.k)
        .key(key)
        .help(tip)
        .preserve_focus_on_press(true)
}

/// Footer toggle button that shows or hides the composer formatting bar.
pub fn toggle_button(s: &Desktop, t: &Theme) -> El {
    let active = s.widgets.format_bar_open || has_formatting(&s.widgets.draft);
    row()
        .key("composer-format-toggle")
        .role("button")
        .label("Formatting")
        .help("Toggle formatting toolbar")
        .h(crate::theme::Size::Compact.height())
        .px(S8)
        .center()
        .radius(crate::theme::RADIUS_CONTROL)
        .fill(if active {
            t.hover_wash
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .focusable(true)
        .focus_ring(crate::theme::HAIRLINE, -crate::theme::HAIRLINE, t.accent)
        .preserve_focus_on_press(true)
        .on_press(|s: &mut Desktop| {
            s.widgets.format_bar_open = !s.widgets.format_bar_open;
        })
        .child(
            text("Aa")
                .font(t.small_medium)
                .color(if active { t.text } else { t.muted })
                .a11y_hidden(),
        )
}

/// Serializes `edit`'s inline marks and paragraph blocks into GitHub-Flavored
/// Markdown, returning `None` when the field contains only plain text and
/// mention pills.
#[must_use]
pub fn to_markdown(edit: &TextEdit) -> Option<String> {
    if !has_formatting(edit) {
        return None;
    }
    let paras = edit.paragraphs();
    let mut out = String::with_capacity(edit.text.len() + 32);
    let mut i = 0;
    let mut numbered_counters = [0usize; 8];
    while i < paras.len() {
        if i > 0 {
            out.push('\n');
        }
        let para = &paras[i];
        match para.block {
            Block::Code => {
                numbered_counters.fill(0);
                out.push_str("```\n");
                loop {
                    let line = &edit.text[paras[i].range.clone()];
                    out.push_str(line);
                    out.push('\n');
                    if i + 1 < paras.len() && paras[i + 1].block == Block::Code {
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push_str("```");
            }
            Block::Bullet(level) => {
                numbered_counters.fill(0);
                for _ in 0..usize::from(level) {
                    out.push_str("  ");
                }
                out.push_str("- ");
                write_inline_markdown(edit, para.range.start, para.range.end, &mut out);
            }
            Block::Numbered(level) => {
                let idx = usize::from(level).min(numbered_counters.len() - 1);
                numbered_counters[idx] += 1;
                for slot in &mut numbered_counters[idx + 1..] {
                    *slot = 0;
                }
                for _ in 0..usize::from(level) {
                    out.push_str("  ");
                }
                let _ = write!(out, "{}. ", numbered_counters[idx]);
                write_inline_markdown(edit, para.range.start, para.range.end, &mut out);
            }
            Block::Quote => {
                numbered_counters.fill(0);
                out.push_str("> ");
                write_inline_markdown(edit, para.range.start, para.range.end, &mut out);
            }
            Block::Paragraph => {
                numbered_counters.fill(0);
                write_inline_markdown(edit, para.range.start, para.range.end, &mut out);
            }
        }
        i += 1;
    }
    Some(out)
}

fn write_inline_markdown(edit: &TextEdit, start: usize, end: usize, out: &mut String) {
    let mut cuts = vec![start, end];
    let marks: Vec<_> = edit
        .marks()
        .iter()
        .filter(|m| !matches!(m.mark, Mark::Pill(_)) && m.start < end && m.end > start)
        .collect();
    for m in &marks {
        cuts.push(m.start.clamp(start, end));
        cuts.push(m.end.clamp(start, end));
    }
    cuts.sort_unstable();
    cuts.dedup();
    for window in cuts.windows(2) {
        let (a, b) = (window[0], window[1]);
        if a >= b {
            continue;
        }
        let slice = &edit.text[a..b];
        let mut flags = InlineFlags::default();
        let mut link: Option<&str> = None;
        for m in &marks {
            if m.start <= a && m.end >= b {
                match &m.mark {
                    Mark::Bold => flags.set(InlineFlags::BOLD),
                    Mark::Italic => flags.set(InlineFlags::ITALIC),
                    Mark::Strike => flags.set(InlineFlags::STRIKE),
                    Mark::Code => flags.set(InlineFlags::CODE),
                    Mark::Link(url) => link = Some(url.as_str()),
                    _ => {}
                }
            }
        }
        if let Some(url) = link {
            out.push('[');
            write_styled_run(slice, flags, out);
            out.push_str("](");
            out.push_str(url);
            out.push(')');
        } else {
            write_styled_run(slice, flags, out);
        }
    }
}

#[derive(Default, Clone, Copy)]
struct InlineFlags(u8);

impl InlineFlags {
    const BOLD: u8 = 1 << 0;
    const CODE: u8 = 1 << 3;
    const ITALIC: u8 = 1 << 1;
    const STRIKE: u8 = 1 << 2;

    const fn set(&mut self, bit: u8) {
        self.0 |= bit;
    }

    const fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

fn write_styled_run(slice: &str, flags: InlineFlags, out: &mut String) {
    if flags.has(InlineFlags::STRIKE) {
        out.push_str("~~");
    }
    if flags.has(InlineFlags::BOLD) {
        out.push_str("**");
    }
    if flags.has(InlineFlags::ITALIC) {
        out.push('*');
    }
    if flags.has(InlineFlags::CODE) {
        out.push('`');
    }
    out.push_str(slice);
    if flags.has(InlineFlags::CODE) {
        out.push('`');
    }
    if flags.has(InlineFlags::ITALIC) {
        out.push('*');
    }
    if flags.has(InlineFlags::BOLD) {
        out.push_str("**");
    }
    if flags.has(InlineFlags::STRIKE) {
        out.push_str("~~");
    }
}
