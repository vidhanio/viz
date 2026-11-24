//! Structured file-search and web-search match inspector card for expanded
//! search tool rows.

use rune::{El, ScrollSpec, Tx, col, icon, row, text};
use rune_kit::{ScrollBar, ScrollBarStyle};
use viz_client::protocol::SearchMatch;

use super::file_preview::line_range;
use crate::{
    app::Desktop,
    model::{Tool, plural, relative_path},
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
    let (display, snippet, glyph, link) = match hit {
        SearchMatch::Web { url, snippet } => (url.clone(), snippet, &ui::EXTERNAL_LINK, true),
        SearchMatch::File { target, snippet } => {
            let path = target.path.to_string_lossy();
            let rel = relative_path(&s.model, &path);
            let display = target
                .location
                .line
                .map_or_else(|| rel.to_owned(), |line| format!("{rel}:{line}"));
            (display, snippet, rune_kit::file_icon(&path), false)
        }
    };
    let mut r = row()
        .key(format!("hit-{index}"))
        .w_full()
        .px(S6)
        .py(S4)
        .gap(S8)
        .center_y()
        .radius(RADIUS_SM)
        .child(icon(glyph, ICON_SMALL).color(t.muted).a11y_hidden())
        .child(
            text(display)
                .font(t.mono_small)
                .color(if link { t.accent_text } else { t.text })
                .truncate()
                .shrink(),
        );
    if !snippet.is_empty() {
        r = r.child(
            text(snippet)
                .font(t.mono_small)
                .color(t.muted)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        );
    }
    if let SearchMatch::Web { url, .. } = hit {
        let url = url.clone();
        r = r
            .role("link")
            .label(&url)
            .hover(t.hover_wash)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.clone())), tx);
            });
    } else if let SearchMatch::File { target, .. } = hit {
        let path = target.path.to_string_lossy().into_owned();
        let lines = line_range(target.location);
        r = r
            .role("button")
            .label(format!("Preview {path}"))
            .hover(t.hover_wash)
            .focusable(true)
            .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::OpenFilePreview {
                        path: path.clone(),
                        lines,
                    }),
                    tx,
                );
            });
    }
    r
}
