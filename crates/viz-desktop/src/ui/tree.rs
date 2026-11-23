//! Legacy sidebar tree rows and trailing hover slots retained while the
//! sidebar overhaul lands in Phase 1.

use rune::{Color, El, col, el, row, text};

use super::{lead, section};
use crate::theme::{
    HAIRLINE, RADIUS_CONTROL, ROW_INSET, S8, S12, TRAILING, TREE_ROW, TREE_ROW_TALL, TREE_STEP,
    Theme,
};

/// A sidebar tree row. Labels start at `ROW_INSET` plus one `TREE_STEP` per
/// depth, only the label shrinks, and the row ends in its [`trailing`] slot.
pub struct TreeLine<'a> {
    pub key: &'a str,
    pub label: &'a str,
    /// A quiet second line; the row grows to `TREE_ROW_TALL`.
    pub detail: Option<&'a str>,
    pub depth: u16,
    /// Unread: full ink.
    pub strong: bool,
    /// Archived: faint.
    pub dim: bool,
    pub selected: bool,
    /// The label's ink at rest: `muted` for most rows, `text` for a project's
    /// header.
    pub ink: Color,
}

impl TreeLine<'_> {
    /// Builds the tree item row against `t`.
    pub fn build(self, leading: Option<El>, slot: El, t: &Theme) -> El {
        let color = if self.dim {
            t.faint
        } else if self.selected || self.strong {
            t.text
        } else {
            self.ink
        };
        let hover_color = if self.dim { t.muted } else { t.text };
        let title = text(self.label)
            .font(if self.selected { t.ui_medium } else { t.ui })
            .color(color)
            .group_color(hover_color)
            .truncate()
            .help_when_truncated(self.label);
        let second = self.detail.map(|detail| {
            text(detail)
                .font(t.small)
                .color(t.faint)
                .truncate()
                .help_when_truncated(detail)
        });
        let has_second = second.is_some();
        let labels = col()
            .min_w(0.0)
            .shrink()
            .grow(1.0)
            .a11y_hidden()
            .child(title)
            .children(second);
        let height = if has_second { TREE_ROW_TALL } else { TREE_ROW };
        let mut line = row()
            .key(self.key)
            .role("treeitem")
            .label(self.label)
            .level(u32::from(self.depth) + 1)
            .h(height)
            .w_full()
            .pl(f32::from(self.depth).mul_add(TREE_STEP, ROW_INSET))
            .gap(S8)
            .center_y()
            .radius(RADIUS_CONTROL)
            .fill(if self.selected {
                t.selected
            } else {
                Color::TRANSPARENT
            })
            .hover_group()
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .children(leading.map(|mark| lead(mark, TREE_ROW)))
            .child(labels)
            .child(slot);
        if let Some(detail) = self.detail {
            line = line.description(detail);
        }
        line
    }
}

/// A row's fixed trailing slot. `rest` shows until the row is hovered or
/// focused, then `action` takes its place; `reveal` keeps the action visible.
pub fn trailing(rest: Option<El>, action: Option<El>, reveal: bool) -> El {
    let layer = |child: El| {
        el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .center()
            .child(child)
    };
    let mut slot = el().key("trailing").size(TRAILING, TRAILING);
    if let Some(rest) = rest {
        let hovered = if action.is_some() { 0.0 } else { 1.0 };
        let shown = if reveal { 0.0 } else { 1.0 };
        slot = slot.child(layer(rest).group_opacity(shown, hovered));
    }
    if let Some(action) = action {
        slot = slot.child(layer(action).group_opacity(if reveal { 1.0 } else { 0.0 }, 1.0));
    }
    slot
}

/// A sidebar section's title row, with an optional action in the trailing slot.
pub fn section_row(key: &str, label: &str, action: Option<El>, t: &Theme) -> El {
    row()
        .key(key)
        .w_full()
        .h(TREE_ROW)
        .mt(S12)
        .pl(ROW_INSET)
        .gap(S8)
        .center_y()
        .child(section(label, t).grow(1.0))
        .child(trailing(None, action, true))
}
