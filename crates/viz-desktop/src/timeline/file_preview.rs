//! Centered file and attachment preview modal (with syntax highlighting, line
//! numbers, `#Lstart-Lend` accent highlight + auto-scroll, and zoomable image
//! lightbox for images), plus `file://` / `viz-attachment://` link parsers.

use std::path::Path;

use rune::{Color, El, ScrollSpec, Tx, col, el, icon, rich, row, text};
use rune_kit::{IconButtonVariant, Preview, ScrollBar, ScrollBarStyle, file_icon};
use viz_client::protocol::ThreadId;

use crate::{
    app::Desktop,
    attachments::compact_bytes,
    effects::Effect,
    model::{FilePreview, FilePreviewBody, PreviewLines, relative_path, split_path},
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

/// Formats a file URI for use in timeline links.
#[must_use]
pub fn path_to_file_url(path: &str) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// Converts a normalized location to the preview's line index type.
#[must_use]
pub(super) fn line_range(location: viz_client::protocol::Location) -> Option<(usize, usize)> {
    let start = usize::try_from(location.line?).ok()?;
    let end = usize::try_from(location.end_line?).ok()?;
    Some((start, end))
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
        FilePreviewBody::Text { lines, .. } => {
            code_body(preview, lines, body_max_h, Theme::get(t.k.dark))
        }
        FilePreviewBody::EntryText { rows, .. } => entry_body(
            preview.request,
            rows.len(),
            body_max_h,
            Theme::get(t.k.dark),
        ),
        FilePreviewBody::Binary { mime, size } => binary_body(&preview.path, mime, *size, t),
        FilePreviewBody::Error(message) => error_body(message, t),
        FilePreviewBody::Image { .. } => unreachable!(),
    };

    let sheet = t
        .overlay
        .elevate(
            col()
                .key(format!("file-preview-sheet-{}", preview.request))
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

fn entry_body(request: u64, count: usize, max_h: f32, t: &'static Theme) -> El {
    let body = motion::fold_container(col().key("entry-output-rows").w_full().padding(S12))
        .virtual_rows(
            count,
            220.0,
            |_: &Desktop, index| format!("output-{index}").into(),
            move |s: &Desktop, index| {
                if let Some(FilePreviewBody::EntryText { text: source, rows }) = s
                    .model
                    .timeline
                    .file_preview
                    .as_ref()
                    .filter(|preview| preview.request == request)
                    .map(|preview| &preview.body)
                    && let Some(bytes) = rows.get(index)
                {
                    let source = &source[bytes.clone()];
                    let source = source.strip_suffix('\n').unwrap_or(source);
                    let (plain, spans) = super::ansi::parse_ansi(source, t);
                    return rich(plain, spans)
                        .font(t.mono_small)
                        .color(t.text)
                        .w_full()
                        .wrap_anywhere()
                        .selectable(true)
                        .selection_color(t.k.selection());
                }
                el()
            },
        )
        .virtual_rows_version(request);
    col()
        .key(format!("entry-output-{request}"))
        .w_full()
        .max_h(max_h)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(body)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
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
    layer.push(viewer.children(file_action(s, preview, t).map(|button| {
        row()
            .pin(None, Some(S16), Some(S16), None)
            .padding(S4)
            .radius(RADIUS_SM)
            .fill(t.surface_raised)
            .child(button)
    })));
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
    let request = preview.request;
    let is_output = matches!(&preview.body, FilePreviewBody::EntryText { .. });

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
        FilePreviewBody::EntryText { text, .. } => Some(text.len()),
        FilePreviewBody::Loading | FilePreviewBody::Error(_) => None,
    } {
        bar = bar.child(text(compact_bytes(size)).font(t.mono_micro).color(t.faint));
    }

    bar = bar.child(
        rune_kit::IconButton::new(if copied { &ui::CHECK } else { &ui::COPY })
            .help(if is_output {
                "Copy full text"
            } else {
                "Copy path"
            })
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Compact.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                let source = if is_output {
                    match s
                        .model
                        .timeline
                        .file_preview
                        .as_ref()
                        .filter(|p| p.request == request)
                        .map(|p| &p.body)
                    {
                        Some(FilePreviewBody::EntryText { text, .. }) => text.clone(),
                        _ => return,
                    }
                } else {
                    copy_path.clone()
                };
                s.dispatch(
                    Msg::Timeline(TimelineMsg::CopyWithFeedback {
                        key: copy_key.clone(),
                        text: source,
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("copy-path"),
    );

    bar = bar.children(file_action(s, preview, t));

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

fn file_action(s: &Desktop, preview: &FilePreview, t: &Theme) -> Option<El> {
    let file = preview.source.as_ref()?;
    if !s.model.connected()
        || file.session != s.model.connection.live.id
        || matches!(
            preview.body,
            FilePreviewBody::Loading | FilePreviewBody::Error(_)
        )
    {
        return None;
    }
    let local = file.local_root.is_some();
    let (glyph, label) = if local {
        (&ui::EXTERNAL_LINK, "Open local file")
    } else {
        (&ui::DOWNLOAD, "Download file")
    };
    let request = preview.request;
    Some(
        rune_kit::IconButton::new(glyph)
            .help(label)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Compact.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                let msg = if local {
                    TimelineMsg::OpenLocalFile(request)
                } else {
                    TimelineMsg::DownloadFile(request)
                };
                s.dispatch(Msg::Timeline(msg), tx);
            })
            .build(&t.k)
            .key("preview-file-action"),
    )
}

const LINE_HEIGHT: f32 = 20.0;

/// Positions a newly loaded line window at the top of the viewport.
#[expect(
    clippy::cast_precision_loss,
    reason = "scroll layout uses f32 coordinates"
)]
pub fn scroll_to_line(index: usize) -> Effect {
    Effect::ScrollToInstant {
        key: PREVIEW_SCROLL_KEY,
        x: 0.0,
        y: index as f32 * LINE_HEIGHT,
    }
}

fn code_body(preview: &FilePreview, lines: &PreviewLines, max_h: f32, t: &'static Theme) -> El {
    let request = preview.request;
    let range = preview.lines;
    let digits = lines.count.to_string().len().max(3);
    #[expect(clippy::cast_precision_loss, reason = "gutter digit count is tiny")]
    let gutter = (digits as f32).mul_add(7.5, 16.0);
    let rows = motion::fold_container(col().key("preview-lines").w_full())
        .virtual_rows(
            lines.count,
            LINE_HEIGHT,
            |_: &Desktop, index| format!("preview-line-{}", index + 1).into(),
            move |s: &Desktop, index| code_line(s, request, index, range, gutter, t),
        )
        .virtual_rows_version((request, lines.start, lines.rows.len(), digits))
        .on_visible_rows(move |s: &mut Desktop, rows| {
            let Some(preview) = s.model.timeline.file_preview.as_ref() else {
                return false;
            };
            if preview.request == request
                && preview.source.is_some()
                && let FilePreviewBody::Text { lines, .. } = &preview.body
                && (lines.pending || lines.next_start(&rows).is_some())
            {
                s.dispatch_local(Msg::Timeline(TimelineMsg::FilePreviewVisible {
                    request,
                    rows,
                }));
            }
            false
        });
    col()
        .key(PREVIEW_SCROLL_KEY)
        .w_full()
        .max_h(max_h)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(rows)
        .scroll_bar_styled(ScrollBarStyle::scroll_view(&t.k))
}

fn code_line(
    s: &Desktop,
    request: u64,
    index: usize,
    range: Option<(usize, usize)>,
    gutter: f32,
    t: &Theme,
) -> El {
    let line_no = index + 1;
    let selected = range.is_some_and(|(start, end)| (start..=end).contains(&line_no));
    let line = s
        .model
        .timeline
        .file_preview
        .as_ref()
        .filter(|preview| preview.request == request)
        .and_then(|preview| match &preview.body {
            FilePreviewBody::Text { lines, .. } => lines.rows.get(index.checked_sub(lines.start)?),
            _ => None,
        });
    row()
        .w_full()
        .min_h(LINE_HEIGHT)
        .pr(S12)
        .gap(S8)
        .items_stretch()
        .fill(if selected {
            t.accent.alpha(if t.k.dark { 0.15 } else { 0.10 })
        } else {
            Color::TRANSPARENT
        })
        .child(el().w(S2).fill(if selected {
            t.accent
        } else {
            Color::TRANSPARENT
        }))
        .child(
            row().w(gutter).justify_end().pr(S4).a11y_hidden().child(
                text(line_no.to_string())
                    .py(2.0)
                    .font(t.mono_micro)
                    .color(if selected { t.accent_text } else { t.faint }),
            ),
        )
        .children(line.map(|line| {
            rich(&line.text, t.syntax.spans(&line.highlights, 0))
                .font(t.mono_small)
                .py(2.0)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .wrap_anywhere()
                .selectable(true)
                .selection_color(t.k.selection())
        }))
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

pub fn language_for_path(path: &str) -> Option<&'static str> {
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
