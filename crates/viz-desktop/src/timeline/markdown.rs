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

/// Configures `md` with clickable `file://` / `viz-file://` file pills and
/// `viz-attachment://` attachment pills in viz's flat neutral chip style,
/// routing every link click to [`TimelineMsg::OpenUrl`].
pub fn with_file_pills(md: Markdown, cwd: Option<&str>, t: &Theme) -> Markdown {
    let text = t.text;
    let text_strong = t.text_strong;
    let accent_text = t.accent_text;
    let dark = t.k.dark;
    let cwd_owned = cwd.map(|p| p.trim_end_matches('/').to_owned());

    md.on_link_tx(|s: &mut Desktop, url: &str, tx: &mut Tx| {
        s.dispatch(Msg::Timeline(TimelineMsg::OpenUrl(url.to_owned())), tx);
    })
    .link_style(move |url| pill_style(url, text, text_strong, accent_text, dark))
    .link_help(move |url| pill_help(url, cwd_owned.as_deref()))
}

fn pill_style(
    url: &str,
    text: Color,
    text_strong: Color,
    accent_text: Color,
    dark: bool,
) -> Option<(Span, LinkPaint)> {
    if url.starts_with("file://")
        || url.starts_with("viz-file://")
        || url.starts_with("viz-attachment://")
    {
        let rest_bg = text.alpha(if dark { 0.08 } else { 0.06 });
        let hover_bg = text.alpha(if dark { 0.14 } else { 0.11 });
        let span = Span::new(0, 0)
            .color(text_strong)
            .weight(500.0)
            .bg(rest_bg, 2.0);
        let paint = LinkPaint::new().color(accent_text).bg(hover_bg);
        return Some((span, paint));
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
