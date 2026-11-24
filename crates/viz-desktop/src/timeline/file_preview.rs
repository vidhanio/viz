//! Centered file and attachment preview modal (with syntax highlighting, line
//! numbers, `#Lstart-Lend` accent highlight + auto-scroll, and zoomable image
//! lightbox for images), plus `file://` / `viz-attachment://` link parsers.

use std::path::Path;

use rune::{Color, El, ScrollFade, ScrollSpec, Tx, col, el, icon, rich, row, text};
use rune_kit::{
    IconButtonVariant, Preview, ScrollBar, ScrollBarStyle, file_icon, syntax::highlight,
};
use viz_client::protocol::ThreadId;

use crate::{
    app::Desktop,
    attachments::compact_bytes,
    model::{FilePreview, FilePreviewBody, relative_path, split_path},
    motion,
    theme::{
        HAIRLINE, ICON, ICON_MD, ICON_SMALL, RADIUS_CARD, RADIUS_SM, S2, S4, S6, S8, S12, S16,
        Size, Theme,
    },
    ui,
    update::{Msg, TimelineMsg},
};

/// Key of the code preview's vertical scroll viewport.
pub const PREVIEW_SCROLL_KEY: &str = "file-preview-scroll";
/// Maximum width of the code preview modal sheet (points).
const MODAL_MAX_W: f32 = 880.0;
/// Maximum height of the code preview modal sheet (points).
const MODAL_MAX_H: f32 = 720.0;

/// Parses a `file://` or workspace-relative `viz-file://` URL into
/// `(path, Option<(start_line, end_line)>)`, decoding percent-escapes and
/// `#L10` / `#L10-L25` / `:10` / `:10-25` fragments.
#[must_use]
pub fn parse_file_link(raw: &str) -> Option<(String, Option<(usize, usize)>)> {
    if let Some(rest) = raw.strip_prefix("viz-file://") {
        let (path_part, frag) = rest
            .split_once('#')
            .map_or((rest, None), |(p, f)| (p, Some(f)));
        let mut lines = frag.and_then(parse_line_fragment);
        let mut path = percent_decode(path_part);
        if lines.is_none()
            && let Some((stripped, suffix_lines)) = split_colon_lines(&path)
        {
            let len = stripped.len();
            path.truncate(len);
            lines = Some(suffix_lines);
        }
        return (!path.is_empty()).then_some((path, lines));
    }
    if !raw.starts_with("file://") {
        return None;
    }
    let parsed = url::Url::parse(raw).ok()?;
    let mut lines = parsed.fragment().and_then(parse_line_fragment);
    let mut path = parsed
        .to_file_path()
        .ok()
        .map_or_else(|| parsed.path().to_owned(), |p| p.display().to_string());
    if lines.is_none()
        && let Some((stripped, suffix_lines)) = split_colon_lines(&path)
    {
        let len = stripped.len();
        path.truncate(len);
        lines = Some(suffix_lines);
    }
    (!path.is_empty()).then_some((path, lines))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

const fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parses a `viz-attachment://<thread>/<id>` URL into `(ThreadId, id)`.
#[must_use]
pub fn parse_attachment_link(raw: &str) -> Option<(ThreadId, String)> {
    let rest = raw.strip_prefix("viz-attachment://")?;
    let (thread_part, id_part) = rest.split_once('/')?;
    let id = id_part.split(['#', '?']).next().unwrap_or(id_part);
    let thread = thread_part.parse::<u64>().ok()?;
    (!id.is_empty()).then_some((ThreadId(thread), id.to_owned()))
}

/// Formats a `file://` URI for an absolute local path, or returns `None` for
/// relative paths.
#[must_use]
pub fn path_to_file_url(path: &str) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// Extracts a 1-based inclusive line range from JSON tool arguments such as
/// `read_file` (`StartLine`/`EndLine`, `start_line`/`end_line`, or
/// `offset`/`limit`).
#[must_use]
pub fn extract_tool_line_range(raw_input: Option<&str>) -> Option<(usize, usize)> {
    let value: serde_json::Value = serde_json::from_str(raw_input?).ok()?;
    let obj = value.as_object()?;
    let get_num = |keys: &[&str]| -> Option<usize> {
        for key in keys {
            if let Some(n) = obj.get(*key).and_then(serde_json::Value::as_u64) {
                return usize::try_from(n).ok().filter(|&v| v > 0);
            }
        }
        None
    };
    if let Some(start) = get_num(&["StartLine", "start_line", "startLine", "line"]) {
        let end = get_num(&["EndLine", "end_line", "endLine"]).unwrap_or(start);
        return Some((start, end.max(start)));
    }
    if let Some(offset) = get_num(&["offset", "Offset"]) {
        let limit = get_num(&["limit", "Limit"]).unwrap_or(1);
        return Some((offset, offset.saturating_add(limit.saturating_sub(1))));
    }
    None
}

fn parse_line_fragment(fragment: &str) -> Option<(usize, usize)> {
    let trimmed = fragment.trim().trim_start_matches(['L', 'l']);
    if let Some((a, b)) = trimmed.split_once('-') {
        let start = a.trim_start_matches(['L', 'l']).parse::<usize>().ok()?;
        let end = b.trim_start_matches(['L', 'l']).parse::<usize>().ok()?;
        return (start > 0).then_some((start, end.max(start)));
    }
    let start = trimmed.split(':').next()?.parse::<usize>().ok()?;
    (start > 0).then_some((start, start))
}

fn split_colon_lines(path: &str) -> Option<(&str, (usize, usize))> {
    let (prefix, tail) = path.rsplit_once(':')?;
    if prefix.is_empty() || tail.is_empty() {
        return None;
    }
    if let Some((a, b)) = tail.split_once('-')
        && let (Ok(start), Ok(end)) = (a.parse::<usize>(), b.parse::<usize>())
        && start > 0
    {
        return Some((prefix, (start, end.max(start))));
    }
    if let Ok(line) = tail.parse::<usize>()
        && line > 0
    {
        if let Some((outer_prefix, prev_tail)) = prefix.rsplit_once(':')
            && let Ok(outer_line) = prev_tail.parse::<usize>()
            && outer_line > 0
        {
            return Some((outer_prefix, (outer_line, outer_line)));
        }
        return Some((prefix, (line, line)));
    }
    None
}

/// Mounts the file or image preview modal into `layer` when open.
pub fn modal(s: &Desktop, reduced: bool, t: &Theme, layer: &mut Vec<El>) {
    let Some(preview) = &s.model.timeline.file_preview else {
        return;
    };
    if matches!(preview.body, FilePreviewBody::Image { .. }) {
        mount_image_lightbox(s, preview, reduced, t, layer);
        return;
    }

    let window_w = s.model.panels.window_width.max(360.0);
    let window_h = s.model.panels.window_height.max(320.0);
    let card_w = (window_w - 64.0).clamp(320.0, MODAL_MAX_W);
    let card_h = (window_h - 80.0).clamp(240.0, MODAL_MAX_H);

    layer.push(
        el().key("file-preview-scrim")
            .role("none")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(t.scrim)
            .transition(motion::scrim())
            .on_press_tx(|s: &mut Desktop, tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::CloseFilePreview), tx);
            }),
    );

    let body_max_h = (card_h - 48.0).max(160.0);
    let body = match &preview.body {
        FilePreviewBody::Loading => loading_body(t),
        FilePreviewBody::Text {
            text: source,
            start_line,
            truncated,
            ..
        } => code_body(preview, source, *start_line, *truncated, body_max_h, t),
        FilePreviewBody::Binary { mime, size } => binary_body(&preview.path, mime, *size, t),
        FilePreviewBody::Error(message) => error_body(message, t),
        FilePreviewBody::Image { .. } => unreachable!(),
    };

    let sheet = t
        .overlay
        .elevate(
            col()
                .key("file-preview-sheet")
                .role("dialog")
                .label(format!("Preview {}", preview.path))
                .w(card_w)
                .max_h(card_h)
                .radius(RADIUS_CARD)
                .clip_rounded(RADIUS_CARD)
                .fill(t.surface_raised)
                .child(header(s, preview, t))
                .child(ui::rule(t))
                .child(body),
            &t.overlay.elevation.overlay,
        )
        .modal(true, |s: &mut Desktop| {
            s.dispatch_local(Msg::Timeline(TimelineMsg::CloseFilePreview));
        });

    let centered = col()
        .key("file-preview-center")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .center()
        .padding(S16)
        .child(sheet);
    layer.push(centered);
}

fn mount_image_lightbox(
    s: &Desktop,
    preview: &FilePreview,
    reduced: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let (natural, picture) = s
        .preview_image()
        .filter(|img| img.is_ready() && img.error().is_none())
        .map_or_else(
            || {
                (
                    [320.0, 240.0],
                    col()
                        .key("file-preview-img-loading")
                        .center()
                        .gap(S6)
                        .fill(t.surface_raised)
                        .child(ui::spinner(t))
                        .child(text(&preview.path).font(t.ui).color(t.muted)),
                )
            },
            |image| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "decoded image dimensions fit cleanly in f32 lightbox sizing"
                )]
                let dims = image
                    .size()
                    .map_or([320.0, 240.0], |[w, h]| [w as f32, h as f32]);
                (
                    dims,
                    el().key(format!("file-preview-img-{}", preview.request))
                        .pixels(image.pixels()),
                )
            },
        );
    let window = [s.model.panels.window_width, s.model.panels.window_height];
    layer.push(
        el().key("file-preview-img-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(t.scrim)
            .transition(motion::scrim()),
    );
    let viewer = Preview::new(
        |s: &mut Desktop| &mut s.widgets.image_zoom,
        natural,
        picture,
    )
    .icons(&ui::MINUS, &ui::PLUS)
    .on_close(|s: &mut Desktop, tx| {
        s.dispatch(Msg::Timeline(TimelineMsg::CloseFilePreview), tx);
    })
    .build(&s.widgets.image_zoom, &t.overlay, window, reduced);
    layer.push(viewer);
}

#[expect(
    clippy::too_many_lines,
    reason = "single preview sheet header bar with path, line badge, size, and actions"
)]
fn header(s: &Desktop, preview: &FilePreview, t: &Theme) -> El {
    let rel = relative_path(&s.model, &preview.path);
    let (dir, name) = split_path(rel);
    let copy_key = format!("preview-copy-{}", preview.request);
    let copied = s.model.timeline.copied_key.as_deref() == Some(copy_key.as_str());
    let copy_path = preview.path.clone();

    let mut bar = row()
        .key("file-preview-header")
        .w_full()
        .px(S12)
        .py(S8)
        .gap(S8)
        .center_y()
        .fill(t.surface)
        .child(icon(file_icon(name), ICON_MD).color(t.muted).a11y_hidden())
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(S6)
                .center_y()
                .child(
                    text(name)
                        .font(t.ui_medium)
                        .color(t.text_strong)
                        .shrink()
                        .min_w(0.0)
                        .truncate(),
                )
                .children((!dir.is_empty()).then(|| {
                    text(dir)
                        .font(t.small)
                        .color(t.faint)
                        .shrink()
                        .min_w(0.0)
                        .truncate()
                })),
        );

    if let Some((start, end)) = preview.lines {
        let label = if start == end {
            format!("L{start}")
        } else {
            format!("L{start}–L{end}")
        };
        bar = bar.child(
            row()
                .key("line-badge")
                .px(S6)
                .py(S2)
                .radius(RADIUS_SM)
                .fill(t.chip_fill)
                .border(HAIRLINE, t.hairline)
                .child(text(label).font(t.mono_micro).color(t.text_secondary)),
        );
    }

    if let Some(size) = match &preview.body {
        FilePreviewBody::Text { size, .. }
        | FilePreviewBody::Binary { size, .. }
        | FilePreviewBody::Image { size, .. } => usize::try_from(*size).ok(),
        FilePreviewBody::Loading | FilePreviewBody::Error(_) => None,
    } {
        bar = bar.child(text(compact_bytes(size)).font(t.mono_micro).color(t.faint));
    }

    bar = bar.child(
        rune_kit::IconButton::new(if copied { &ui::CHECK } else { &ui::COPY })
            .help("Copy path")
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Compact.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_key.clone(),
                        text: copy_path.clone(),
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("copy-path"),
    );

    if let Some(url) = &preview.open_url {
        let external = url.clone();
        bar = bar.child(
            rune_kit::IconButton::new(&ui::EXTERNAL_LINK)
                .help("Open externally")
                .variant(IconButtonVariant::GhostMuted)
                .control_size(Size::Compact.kit())
                .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                    s.dispatch(
                        Msg::Timeline(TimelineMsg::OpenExternalUrl(external.clone())),
                        tx,
                    );
                })
                .build(&t.k)
                .key("open-external"),
        );
    }

    bar.child(
        rune_kit::IconButton::new(&ui::CLOSE)
            .help("Close (Esc)")
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Compact.kit())
            .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::CloseFilePreview), tx);
            })
            .build(&t.k)
            .key("close-preview"),
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "line-by-line syntax span slicing, line gutter, and highlight band layout"
)]
fn code_body(
    preview: &FilePreview,
    source: &str,
    start_line: usize,
    truncated: bool,
    max_h: f32,
    t: &Theme,
) -> El {
    let lang = language_for_path(&preview.path);
    let highlights = highlight(lang, source);
    let all_spans = t.syntax.spans(&highlights, 0);

    let trimmed_source = source.strip_suffix('\n').unwrap_or(source);
    let line_count = trimmed_source.split('\n').count().max(1);
    let max_line_no = start_line.saturating_add(line_count).saturating_sub(1);
    let gutter_digits = max_line_no.to_string().len().max(3);
    #[expect(clippy::cast_precision_loss, reason = "gutter digit count is tiny")]
    let gutter_w = (gutter_digits as f32).mul_add(7.5, 16.0);
    let highlight_bg = t.accent.alpha(if t.k.dark { 0.15 } else { 0.10 });

    let mut rows = col().key("preview-lines").w_full().py(S8);
    let mut span_cursor = 0usize;
    let mut offset = 0usize;

    for (idx, raw_line) in trimmed_source.split('\n').enumerate() {
        let line_no = start_line + idx;
        let line_start = offset;
        let line_end = offset + raw_line.len();
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let content_end = line_start + line.len();
        offset = (line_end + 1).min(source.len());

        while span_cursor < all_spans.len() && (all_spans[span_cursor].end as usize) <= line_start {
            span_cursor += 1;
        }
        let mut line_spans = Vec::new();
        let mut k = span_cursor;
        while k < all_spans.len() && (all_spans[k].start as usize) < content_end {
            let sp = &all_spans[k];
            let s = (sp.start as usize).max(line_start) - line_start;
            let e = (sp.end as usize).min(content_end) - line_start;
            if s < e {
                let mut cloned = sp.clone();
                cloned.start = u32::try_from(s).unwrap_or(u32::MAX);
                cloned.end = u32::try_from(e).unwrap_or(u32::MAX);
                line_spans.push(cloned);
            }
            k += 1;
        }

        let is_highlighted = preview
            .lines
            .is_some_and(|(a, b)| line_no >= a && line_no <= b);
        let display = if line.is_empty() { " " } else { line };
        let code_el = if line_spans.is_empty() {
            text(display)
                .font(t.mono_small)
                .color(t.text)
                .wrap_anywhere()
                .selectable(true)
                .selection_color(t.k.selection())
        } else {
            rich(display, line_spans)
                .font(t.mono_small)
                .color(t.text)
                .wrap_anywhere()
                .selectable(true)
                .selection_color(t.k.selection())
        };

        let line_row =
            row()
                .key(format!("preview-line-{line_no}"))
                .w_full()
                .pr(S12)
                .gap(S8)
                .items_stretch()
                .fill(if is_highlighted {
                    highlight_bg
                } else {
                    Color::TRANSPARENT
                })
                .child(
                    el().w(S2)
                        .fill(if is_highlighted {
                            t.accent
                        } else {
                            Color::TRANSPARENT
                        })
                        .a11y_hidden(),
                )
                .child(
                    row()
                        .w(gutter_w)
                        .justify_end()
                        .pr(S4)
                        .py(1.0)
                        .a11y_hidden()
                        .child(text(line_no.to_string()).font(t.mono_micro).color(
                            if is_highlighted {
                                t.accent_text
                            } else {
                                t.faint
                            },
                        )),
                )
                .child(code_el.py(1.0).grow(1.0).shrink().min_w(0.0));

        rows = rows.child(line_row);
    }

    if truncated {
        rows = rows.child(
            row().w_full().px(S16).pt(S8).child(
                text("Preview truncated at 512 KB — open externally to view the full file.")
                    .font(t.micro)
                    .color(t.faint),
            ),
        );
    }

    col()
        .key(PREVIEW_SCROLL_KEY)
        .w_full()
        .max_h(max_h)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_fade(ScrollFade::strength(S16).linear())
        .child(rows)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
}

fn loading_body(t: &Theme) -> El {
    row()
        .key("file-preview-loading")
        .w_full()
        .h(180.0)
        .center()
        .gap(S8)
        .child(ui::spinner(t))
        .child(text("Loading file…").font(t.ui).color(t.muted))
}

fn binary_body(path: &str, mime: &str, size: u64, t: &Theme) -> El {
    let size_text = usize::try_from(size).map_or_else(|_| format!("{size} B"), compact_bytes);
    col()
        .key("file-preview-binary")
        .w_full()
        .py(36.0)
        .px(S16)
        .center()
        .gap(S8)
        .child(
            icon(file_icon(path), ICON + S8)
                .color(t.muted)
                .a11y_hidden(),
        )
        .child(
            text("Binary file preview unavailable")
                .font(t.ui_medium)
                .color(t.text),
        )
        .child(
            text(format!("{mime} · {size_text}"))
                .font(t.small)
                .color(t.faint),
        )
}

fn error_body(message: &str, t: &Theme) -> El {
    col()
        .key("file-preview-error")
        .w_full()
        .py(36.0)
        .px(S16)
        .center()
        .gap(S8)
        .child(
            icon(&ui::TRIANGLE_ALERT, ICON + S4)
                .color(t.danger)
                .a11y_hidden(),
        )
        .child(
            text("Couldn’t preview file")
                .font(t.ui_medium)
                .color(t.text),
        )
        .child(
            text(message)
                .font(t.small)
                .color(t.muted)
                .wrap()
                .max_w(480.0),
        )
}

fn language_for_path(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "js",
        "jsx" => "jsx",
        "py" | "pyi" => "python",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "mdx" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "nix" => "nix",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "css" | "scss" => "css",
        "html" | "htm" | "xml" | "svg" => "html",
        "sql" => "sql",
        "lua" | "luau" => "lua",
        "rb" => "ruby",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "zig" => "zig",
        _ => return None,
    })
}

/// Renders a subtle inline preview icon cue on clickable non-expandable file
/// tool rows (like `Read File`).
pub fn row_preview_cue(t: &Theme) -> El {
    row()
        .key("preview-cue")
        .px(S4)
        .py(S2)
        .radius(RADIUS_SM)
        .group_opacity(0.0, 1.0)
        .a11y_hidden()
        .child(icon(&ui::EXTERNAL_LINK, ICON_SMALL).color(t.faint))
}
