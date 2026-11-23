//! Structured file-search and web-search match inspector card for expanded
//! search tool rows.

use rune::{El, ScrollSpec, Tx, col, icon, row, text};
use rune_kit::{ScrollBar, ScrollBarStyle};

use crate::{
    app::Desktop,
    model::{SearchMatch, Tool, plural, relative_path},
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, RADIUS_SM, S2, S4, S6, S8, S10, Theme},
    ui::{self},
    update::{Msg, TimelineMsg},
};

/// Maximum search match list height (`240pt`).
const SEARCH_MAX_H: f32 = 240.0;

/// Renders a scrollable card of parsed file or URL matches for `tool`.
pub fn search_inspector(s: &Desktop, tool: &Tool, t: &Theme) -> El {
    let header = row()
        .key("search-header")
        .w_full()
        .px(S10)
        .py(S6)
        .gap(S8)
        .center_y()
        .fill(t.surface_recessed)
        .child(
            rune_kit::Badge::new(&tool.summary)
                .variant(rune_kit::BadgeVariant::Subtle)
                .build(&t.k)
                .key("query"),
        )
        .child(
            text(format!(
                "{} {}",
                tool.search_matches.len(),
                plural(tool.search_matches.len(), "match", "matches")
            ))
            .font(t.micro)
            .color(t.faint),
        );

    let list = col().w_full().padding(S6).gap(S2).children(
        tool.search_matches
            .iter()
            .enumerate()
            .map(|(i, hit)| search_hit_row(s, i, hit, t)),
    );

    col()
        .key("search-card")
        .w_full()
        .radius(RADIUS_CONTROL)
        .fill(t.surface)
        .border(HAIRLINE, t.hairline)
        .clip_rounded(RADIUS_CONTROL)
        .child(header)
        .child(ui::rule(t))
        .child(
            col()
                .key("search-scroll")
                .w_full()
                .max_h(SEARCH_MAX_H)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list)
                .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k)),
        )
}

fn search_hit_row(s: &Desktop, index: usize, hit: &SearchMatch, t: &Theme) -> El {
    let display = if hit.is_url {
        hit.target.clone()
    } else {
        let rel = relative_path(&s.model, &hit.target);
        hit.line
            .map_or_else(|| rel.to_owned(), |line| format!("{rel}:{line}"))
    };
    let mut r = row()
        .key(format!("hit-{index}"))
        .w_full()
        .px(S6)
        .py(S4)
        .gap(S8)
        .center_y()
        .radius(RADIUS_SM)
        .child(
            icon(
                if hit.is_url {
                    &ui::EXTERNAL_LINK
                } else {
                    rune_kit::file_icon(&hit.target)
                },
                ICON_SMALL,
            )
            .color(t.muted)
            .a11y_hidden(),
        )
        .child(
            text(display)
                .font(t.mono_small)
                .color(if hit.is_url { t.accent_text } else { t.text })
                .truncate()
                .shrink(),
        );
    if !hit.snippet.is_empty() {
        r = r.child(
            text(&hit.snippet)
                .font(t.mono_small)
                .color(t.muted)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    }
    if hit.is_url {
        let url = hit.target.clone();
        r = r
            .role("link")
            .label(&hit.target)
            .hover(t.hover_wash)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
            });
    }
    r
}
