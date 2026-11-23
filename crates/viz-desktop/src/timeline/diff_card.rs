//! Inline unified diff preview cards for file-modifying tool inspectors, with
//! word-level change highlights and Track G `"Open diff"` seam.

use rune::{El, ScrollSpec, Span, Tx, col, icon, rich, row, text};
use rune_kit::{
    IconButtonVariant, ScrollBar, ScrollBarStyle,
    diff::{FileDiff, LineKind},
};

use crate::{
    app::Desktop,
    model::{Tool, relative_path, split_path},
    theme::{CARD_INSET, HAIRLINE, ICON_MD, RADIUS_CONTROL, S2, S4, S6, S8, S10, Size, Theme},
    ui::{self, InlineStyle},
    update::{Msg, TimelineMsg},
};

/// Maximum inline diff lines drawn inside a tool inspector card.
const MAX_DIFF_LINES: usize = 300;
/// Maximum scroll height for an inline diff preview card (`260pt`).
const DIFF_SCROLL_MAX_H: f32 = 260.0;

/// Renders memoized inline diff cards for every file diff reported by `tool`.
pub fn file_diffs_inspector(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let cwd = s.model.thread_state().map(|state| state.cwd.as_str());
    rune::memo(
        format!("{}-diffs", tool.key),
        (&tool.diffs, t.k.dark, cwd),
        || {
            col()
                .role("group")
                .label("File changes")
                .w_full()
                .gap(S8)
                .children(tool.diffs.iter().enumerate().map(|(index, diff)| {
                    let file = FileDiff::between(
                        diff.old_text.as_ref().map(|_| diff.path.clone()),
                        Some(diff.path.clone()),
                        diff.old_text.as_deref().unwrap_or_default(),
                        &diff.new_text,
                    );
                    diff_card(s, &file, t).key(format!("diff-{index}"))
                }))
        },
    )
}

fn diff_card(s: &Desktop, file: &FileDiff, t: &Theme) -> El {
    let header = diff_header(s, file, t);
    let mut body = col().w_full().py(S4);
    let total: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
    let mut shown = 0;
    for (idx, hunk) in file.hunks.iter().enumerate() {
        if shown >= MAX_DIFF_LINES {
            break;
        }
        if idx > 0 {
            body = body.child(
                text(hunk.header())
                    .font(t.mono_small)
                    .color(t.faint)
                    .px(CARD_INSET)
                    .py(S2),
            );
        }
        for line in hunk.lines.iter().take(MAX_DIFF_LINES - shown) {
            body = body.child(diff_line(line, t));
            shown += 1;
        }
    }
    if total > shown {
        body = body.child(
            text(format!("{} more lines", total - shown))
                .font(t.micro)
                .color(t.faint)
                .px(CARD_INSET)
                .pt(S4),
        );
    }

    col()
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .clip_rounded(RADIUS_CONTROL)
        .child(header)
        .child(ui::rule(t))
        .child(
            col()
                .key("diff-scroll")
                .w_full()
                .max_h(DIFF_SCROLL_MAX_H)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(body)
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k)),
        )
}

fn diff_header(s: &Desktop, file: &FileDiff, t: &Theme) -> El {
    let path = file.path();
    let rel = relative_path(&s.model, path).to_owned();
    let (dir, name) = split_path(&rel);
    let add = u32::try_from(file.additions()).unwrap_or(u32::MAX);
    let del = u32::try_from(file.deletions()).unwrap_or(u32::MAX);
    let open_path = rel.clone();
    let unified = file.patch();

    row()
        .key("diff-head")
        .w_full()
        .px(S10)
        .py(S6)
        .gap(S6)
        .center_y()
        .fill(t.surface_recessed)
        .child(
            icon(rune_kit::file_icon(name), ICON_MD)
                .color(t.muted)
                .a11y_hidden(),
        )
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(S6)
                .center_y()
                .help(path)
                .child(
                    text(name)
                        .font(t.mono_small)
                        .color(t.text)
                        .truncate()
                        .min_w(0.0),
                )
                .children((!dir.is_empty()).then(|| {
                    text(dir.trim_end_matches('/'))
                        .font(t.mono_small)
                        .color(t.faint)
                        .truncate()
                        .shrink()
                        .min_w(0.0)
                })),
        )
        .child(ui::diff_stat(add, del, t))
        .child(
            ui::inline_button("Open diff")
                .icon(&ui::FILE_DIFF)
                .style(InlineStyle::Wash)
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenDiff {
                            path: Some(open_path.clone()),
                        }),
                        tx,
                    );
                })
                .build(t)
                .key("open-diff"),
        )
        .child(
            rune_kit::IconButton::new(&ui::COPY)
                .help("Copy patch")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Micro.kit())
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(Msg::Timeline(TimelineMsg::CopyText(unified.clone())), tx);
                })
                .build(&t.k)
                .key("copy-patch"),
        )
}

fn diff_line(line: &rune_kit::diff::Line, t: &Theme) -> El {
    let (sign, sign_ink, wash, word_wash, ink) = match line.kind {
        LineKind::Added => ("+", t.success, Some(t.added), Some(t.added_word), t.text),
        LineKind::Removed => ("−", t.danger, Some(t.removed), Some(t.removed_word), t.text),
        LineKind::Context => (" ", t.faint, None, None, t.muted),
    };
    let spans = word_wash.map_or_else(Vec::new, |word| {
        line.changes
            .iter()
            .filter(|range| range.start < range.end && range.end <= line.text.len())
            .map(|range| Span {
                bg: Some(word),
                ..Span::new(range.start, range.end)
            })
            .collect()
    });
    let mut r = row()
        .w_full()
        .px(CARD_INSET)
        .gap(S4)
        .items_start()
        .child(
            text(sign)
                .font(t.mono_small)
                .color(sign_ink)
                .w(S10)
                .a11y_hidden(),
        )
        .child(
            rich(line.text.clone(), spans)
                .font(t.mono_small)
                .color(ink)
                .wrap_anywhere()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    if let Some(wash) = wash {
        r = r.fill(wash);
    }
    r
}
