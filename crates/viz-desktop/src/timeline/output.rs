//! Expanded tool output and inline file diff cards.

use rune::{El, ScrollSpec, Span, Tx, col, rich, row, text};
use rune_kit::{
    IconButtonVariant,
    diff::{FileDiff, LineKind},
};

use super::activity::INDENT;
use crate::{
    app::Desktop,
    model::{Tool, ToolAction, ToolStatus, is_command, relative_path, split_path},
    theme::{self, CARD_INSET, S4, S8, S12, S32, Size, Theme},
    ui::{self, Card},
    update::{Msg, TimelineMsg},
};

/// The tallest an expanded tool block's body gets before it scrolls.
const BLOCK_HEIGHT: f32 = 280.0;
/// The most diff lines an expanded edit draws; the copy action still copies
/// the whole patch.
const DIFF_LINES: usize = 400;

/// An open call's output and changes, as cards at [`INDENT`].
pub(super) fn tool_output(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let mut blocks = col().key("body").w_full().pl(INDENT).pt(S4).pb(S8).gap(S8);
    if tool.diffs.is_empty() {
        blocks = blocks.child(output_card(s, tool, t));
    } else {
        let cwd = s.model.thread_state().map(|state| state.cwd.as_str());
        blocks = blocks.child(rune::memo(
            format!("{}-diffs", tool.key),
            (&tool.diffs, t.k.dark, cwd),
            || diffs(s, tool, t),
        ));
    }
    blocks
}

fn card_header(title: El, copy: El) -> El {
    row()
        .key("head")
        .w_full()
        .h(S32)
        .pl(CARD_INSET)
        .pr(S4)
        .gap(S8)
        .center_y()
        .child(title.grow(1.0).shrink().min_w(0.0))
        .child(copy)
}

fn copy_button(label: &str, copied: String, t: &Theme) -> El {
    ui::icon_button(&ui::COPY, label, IconButtonVariant::GhostMuted, Size::Small)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(Msg::Timeline(TimelineMsg::CopyText(copied.clone())), tx);
        })
        .build(&t.k)
        .key("copy")
}

fn card_body(body: El, follow: bool) -> El {
    col()
        .key("scroll")
        .w_full()
        .max_h(BLOCK_HEIGHT)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: follow,
        })
        .child(body)
}

fn output_card(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let command = is_command(&tool.name);
    let action = ToolAction::of(tool);
    let literal = command || action == ToolAction::Read;
    let title = if command {
        let full = tool.input.as_deref().unwrap_or(&tool.summary);
        text(format!("$ {}", full.lines().next().unwrap_or_default()))
            .font(t.mono)
            .color(t.muted)
            .truncate()
            .help_when_truncated(full)
    } else if action.on_file() {
        text(relative_path(&s.model, &tool.summary))
            .font(t.ui)
            .color(t.muted)
            .truncate()
            .help_when_truncated(&tool.summary)
    } else {
        text("Output").font(t.ui).color(t.muted)
    };
    let output = tool.output.trim_end();
    let body = if literal {
        ui::code(output, t)
    } else {
        text(output)
            .w_full()
            .font(t.ui)
            .color(t.text)
            .wrap()
            .selectable(true)
            .selection_color(t.k.selection())
    };
    Card::Surface
        .frame(t)
        .key("output")
        .clip_rounded(theme::RADIUS_CARD)
        .child(card_header(
            title,
            copy_button("Copy output", tool.output.clone(), t),
        ))
        .child(ui::rule(t))
        .child(card_body(
            col().w_full().px(CARD_INSET).py(S8).child(body),
            tool.status == ToolStatus::Running,
        ))
}

fn diffs(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    col()
        .role("group")
        .label("Changes")
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
}

fn diff_card(s: &Desktop, file: &FileDiff, t: &Theme) -> El {
    let path = file.path();
    let (dir, name) = split_path(relative_path(&s.model, path));
    let count = |n: usize, sign: char, ink| {
        (n > 0).then(|| text(format!("{sign}{n}")).font(t.mono_small).color(ink))
    };
    let title = row()
        .gap(S8)
        .center_y()
        .help(path)
        .child(
            text(name)
                .font(t.ui_medium)
                .color(t.text)
                .truncate()
                .min_w(0.0),
        )
        .children((!dir.is_empty()).then(|| {
            text(dir.trim_end_matches('/'))
                .font(t.ui)
                .color(t.faint)
                .truncate()
                .shrink()
                .min_w(0.0)
        }))
        .children(count(file.additions(), '+', t.success))
        .children(count(file.deletions(), '−', t.danger));
    let mut body = col().w_full().py(S4);
    if file.hunks.is_empty() {
        body = body.child(
            text(if file.binary {
                "Binary change"
            } else {
                "No text changes"
            })
            .font(t.ui)
            .color(t.muted)
            .px(CARD_INSET),
        );
    }
    let total: usize = file.hunks.iter().map(|hunk| hunk.lines.len()).sum();
    let mut shown = 0;
    for (index, hunk) in file.hunks.iter().enumerate() {
        if shown == DIFF_LINES {
            break;
        }
        if index > 0 {
            body = body.child(
                text(hunk.header())
                    .font(t.mono_small)
                    .color(t.faint)
                    .px(CARD_INSET)
                    .py(S4),
            );
        }
        for line in hunk.lines.iter().take(DIFF_LINES - shown) {
            body = body.child(diff_line(line, t));
            shown += 1;
        }
    }
    if total > shown {
        body = body.child(
            text(format!("{} more lines", total - shown))
                .font(t.small)
                .color(t.faint)
                .px(CARD_INSET)
                .pt(S4),
        );
    }
    Card::Surface
        .frame(t)
        .clip_rounded(theme::RADIUS_CARD)
        .child(card_header(
            title,
            copy_button("Copy patch", file.patch(), t),
        ))
        .child(ui::rule(t))
        .child(card_body(body, false))
}

fn diff_line(line: &rune_kit::diff::Line, t: &Theme) -> El {
    let (sign, sign_ink, wash, ink) = match line.kind {
        LineKind::Added => ("+", t.success, Some(t.added), t.text),
        LineKind::Removed => ("−", t.danger, Some(t.removed), t.text),
        LineKind::Context => ("", t.faint, None, t.muted),
    };
    let spans = wash.map_or_else(Vec::new, |wash| {
        line.changes
            .iter()
            .filter(|range| range.start < range.end && range.end <= line.text.len())
            .map(|range| Span {
                bg: Some(wash),
                ..Span::new(range.start, range.end)
            })
            .collect()
    });
    let mut row = row()
        .w_full()
        .px(CARD_INSET)
        .gap(S4)
        .items_start()
        .child(text(sign).font(t.mono).color(sign_ink).w(S12).a11y_hidden())
        .child(
            rich(line.text.clone(), spans)
                .font(t.mono)
                .color(ink)
                .wrap_anywhere()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .selectable(true)
                .selection_color(t.k.selection()),
        );
    if let Some(wash) = wash {
        row = row.fill(wash);
    }
    row
}
