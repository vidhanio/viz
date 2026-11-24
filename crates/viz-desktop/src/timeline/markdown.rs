//! Shared `rune_kit::Markdown` link pill styling (`file://`, `viz-file://`,
//! and `viz-attachment://`) and click handlers used across user bubbles,
//! assistant prose, plans, task results, and tool inspectors.

use rune::{Color, LinkPaint, Span, Tx};
use rune_kit::Markdown;

use super::file_preview;
use crate::{
    app::Desktop,
    theme::Theme,
    update::{Msg, TimelineMsg},
};

/// Configures `md` with clickable `file://` / `viz-file://` file pills
/// (matching the composer's `@file` pill aesthetic) and `viz-attachment://`
/// attachment pills, routing every link click to [`TimelineMsg::OpenUrl`].
pub fn with_file_pills(md: Markdown, cwd: Option<&str>, t: &Theme) -> Markdown {
    let info = t.info;
    let accent = t.accent;
    let accent_text = t.accent_text;
    let dark = t.k.dark;
    let cwd_owned = cwd.map(|p| p.trim_end_matches('/').to_owned());

    md.on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
        s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
    })
    .link_style(move |url| pill_style(url, info, accent, accent_text, dark))
    .link_ring(move |url| pill_ring(url, info, accent, dark))
    .link_help(move |url| pill_help(url, cwd_owned.as_deref()))
}

fn pill_style(
    url: &str,
    info: Color,
    accent: Color,
    accent_text: Color,
    dark: bool,
) -> Option<(Span, LinkPaint)> {
    if url.starts_with("file://") || url.starts_with("viz-file://") {
        let rest_bg = info.alpha(if dark { 0.14 } else { 0.10 });
        let hover_bg = info.alpha(if dark { 0.24 } else { 0.18 });
        let hover_ring = info.alpha(if dark { 0.48 } else { 0.40 });
        let span = Span::new(0, 0).color(info).weight(500.0).bg(rest_bg, 3.0);
        let paint = LinkPaint::new()
            .color(info)
            .bg(hover_bg)
            .ring(hover_ring, 1.0);
        return Some((span, paint));
    }
    if url.starts_with("viz-attachment://") {
        let rest_bg = accent.alpha(if dark { 0.15 } else { 0.10 });
        let hover_bg = accent.alpha(if dark { 0.25 } else { 0.18 });
        let hover_ring = accent.alpha(if dark { 0.48 } else { 0.40 });
        let span = Span::new(0, 0)
            .color(accent_text)
            .weight(500.0)
            .bg(rest_bg, 3.0);
        let paint = LinkPaint::new()
            .color(accent_text)
            .bg(hover_bg)
            .ring(hover_ring, 1.0);
        return Some((span, paint));
    }
    None
}

fn pill_ring(url: &str, info: Color, accent: Color, dark: bool) -> Option<(Color, f32)> {
    if url.starts_with("file://") || url.starts_with("viz-file://") {
        return Some((info.alpha(if dark { 0.28 } else { 0.22 }), 1.0));
    }
    if url.starts_with("viz-attachment://") {
        return Some((accent.alpha(if dark { 0.30 } else { 0.24 }), 1.0));
    }
    None
}

fn pill_help(url: &str, cwd: Option<&str>) -> Option<String> {
    if let Some((path, lines)) = file_preview::parse_file_link(url) {
        let rel = cwd
            .and_then(|root| path.strip_prefix(root))
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|rest| !rest.is_empty())
            .unwrap_or(&path);
        return Some(match lines {
            Some((a, b)) if a == b => format!("Preview {rel} (L{a})"),
            Some((a, b)) => format!("Preview {rel} (L{a}–L{b})"),
            None => format!("Preview {rel}"),
        });
    }
    if url.starts_with("viz-attachment://") {
        return Some("Preview attachment".into());
    }
    None
}
