//! Composer attachment strip (Row A `64×64` media thumbnails with hover remove
//! badge, progress/retry states, and `ImageZoom` lightbox; Row B `28px`
//! non-media file rows).

use rune::{El, SurfaceFit, col, el, icon, row, text};
use rune_kit::{IconButtonVariant, Preview};
use viz_client::protocol::Attachment;

use crate::{
    app::Desktop,
    attachments::{compact_bytes, decoded_size},
    motion,
    theme::{
        HAIRLINE, ICON, ICON_MD, ICON_SMALL, RADIUS_CONTROL, RADIUS_PILL, RADIUS_SM, S4, S6, S8,
        S12, S16, S28, Size, Theme,
    },
    ui,
    update::{ComposerMsg, Msg},
};

/// Side length of a Row A media attachment thumbnail tile (`64×64pt`).
const MEDIA_TILE: f32 = 64.0;
/// Diameter of the top-right circular remove badge on a media thumbnail.
const MEDIA_CLOSE: f32 = 18.0;
/// Height of the bottom upload-progress overlay on a media thumbnail.
const PROGRESS_H: f32 = 16.0;

/// Renders the two-row attachment strip inside the top of the composer card
/// when at least one attachment is present.
pub fn strip(s: &Desktop, t: &Theme) -> Option<El> {
    let draft = &s.model.composer.draft;
    if draft.attachments.is_empty() {
        return None;
    }
    let mut media = Vec::new();
    let mut files = Vec::new();
    for (index, attachment) in draft.attachments.iter().enumerate() {
        if attachment.mime.starts_with("image/") {
            media.push((index, attachment));
        } else {
            files.push((index, attachment));
        }
    }
    let mut stack = col()
        .key("draft-attachments")
        .w_full()
        .px(S16)
        .pt(S12)
        .gap(S8);
    if !media.is_empty() {
        stack = stack.child(
            row()
                .key("draft-media-row")
                .w_full()
                .gap(S8)
                .flex_wrap()
                .children(
                    media
                        .into_iter()
                        .map(|(index, file)| media_tile(s, index, file, t)),
                ),
        );
    }
    if !files.is_empty() {
        stack = stack.child(
            col().key("draft-files-row").w_full().gap(S4).children(
                files
                    .into_iter()
                    .map(|(index, file)| file_row(s, index, file, t)),
            ),
        );
    }
    Some(stack)
}

fn media_tile(s: &Desktop, index: usize, attachment: &Attachment, t: &Theme) -> El {
    let decode_error = image_decode_error(s, index);
    let status = decode_error
        .as_ref()
        .unwrap_or_else(|| s.model.composer.draft.attachment_status(index));
    let border = if decode_error.is_some() {
        t.danger
    } else {
        t.hairline
    };
    let preview = s
        .attachment_image(index)
        .filter(|img| img.is_ready() && img.error().is_none())
        .map_or_else(
            || {
                row()
                    .size(MEDIA_TILE, MEDIA_TILE)
                    .center()
                    .fill(t.surface_recessed)
                    .a11y_hidden()
                    .child(icon(&ui::EYE, ICON).color(t.muted))
            },
            |image| {
                el().size(MEDIA_TILE, MEDIA_TILE)
                    .pixels_fit(image.pixels(), SurfaceFit::Fill)
                    .a11y_hidden()
            },
        );
    let mut tile = el()
        .key(format!("attachment-media-{index}"))
        .role("button")
        .label(format!("Preview {}", attachment.name))
        .help(&attachment.name)
        .size(MEDIA_TILE, MEDIA_TILE)
        .radius(RADIUS_CONTROL)
        .clip_rounded(RADIUS_CONTROL)
        .fill(t.surface_recessed)
        .border(HAIRLINE, border)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .transition(motion::chip_enter())
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.widgets.image_zoom.reset();
            s.dispatch(Msg::Composer(ComposerMsg::OpenLightbox(Some(index))), tx);
        })
        .child(preview);

    match status {
        crate::model::AttachmentStatus::Ready => {}
        crate::model::AttachmentStatus::Uploading(pct) => {
            let label = pct.map_or_else(|| "…".to_owned(), |p| format!("{p}%"));
            tile = tile.child(
                row()
                    .key("progress")
                    .pin(Some(0.0), None, Some(0.0), Some(0.0))
                    .h(PROGRESS_H)
                    .center()
                    .fill(t.canvas.alpha(0.85))
                    .a11y_hidden()
                    .child(text(label).font(t.mono_micro).color(t.text_strong)),
            );
        }
        crate::model::AttachmentStatus::Error(message) => {
            tile = tile.child(retry_thumbnail(index, message, t));
        }
    }

    let close = row()
        .key("media-close")
        .role("button")
        .label(format!("Remove {}", attachment.name))
        .help("Remove")
        .pin(None, Some(S4), Some(S4), None)
        .size(MEDIA_CLOSE, MEDIA_CLOSE)
        .center()
        .radius(RADIUS_PILL)
        .fill(t.canvas.alpha(0.85))
        .border(HAIRLINE, t.border_overlay)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Composer(ComposerMsg::RemoveAttachment(index)), tx);
        })
        .child(icon(&ui::CLOSE, 11.0).color(t.text_strong).a11y_hidden());

    tile.child(close)
}

fn file_row(s: &Desktop, index: usize, attachment: &Attachment, t: &Theme) -> El {
    let status = s.model.composer.draft.attachment_status(index);
    let is_error = matches!(status, crate::model::AttachmentStatus::Error(_));
    let bytes = decoded_size(&attachment.data);
    let size_label = (bytes > 0).then(|| compact_bytes(bytes));
    let glyph = rune_kit::file_icon(&attachment.name);
    let border = if is_error { t.danger } else { t.hairline };

    let mut item = row()
        .key(format!("attachment-file-{index}"))
        .w_full()
        .h(S28)
        .px(S8)
        .gap(S8)
        .center_y()
        .radius(RADIUS_SM)
        .fill(t.chip_fill)
        .border(HAIRLINE, border)
        .transition(motion::chip_enter())
        .child(icon(glyph, ICON_MD).color(t.muted).a11y_hidden())
        .child(
            text(&attachment.name)
                .font(t.panel_row)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .help_when_truncated(&attachment.name),
        );

    match status {
        crate::model::AttachmentStatus::Uploading(pct) => {
            let label = pct.map_or_else(|| "Uploading…".to_owned(), |p| format!("{p}%"));
            item = item.child(text(label).font(t.mono_micro).color(t.accent_text));
        }
        crate::model::AttachmentStatus::Error(message) => {
            item = item
                .child(
                    text(message)
                        .font(t.micro)
                        .color(t.danger)
                        .max_w(140.0)
                        .truncate(),
                )
                .child(
                    rune_kit::IconButton::new(&ui::ROTATE_CW)
                        .help("Retry attachment")
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Micro.kit())
                        .on_press_tx(move |s: &mut Desktop, tx| {
                            crate::attachments::retry_draft_image(s, index);
                            tx.wake_after(std::time::Duration::from_millis(32));
                        })
                        .build(&t.k)
                        .key("retry"),
                );
        }
        crate::model::AttachmentStatus::Ready => {
            if let Some(size_label) = size_label {
                item = item.child(
                    text(size_label)
                        .font(t.mono_micro)
                        .color(t.faint)
                        .a11y_hidden(),
                );
            }
        }
    }

    item.child(
        rune_kit::IconButton::new(&ui::CLOSE)
            .help("Remove attachment")
            .variant(IconButtonVariant::GhostMuted)
            .control_size(Size::Micro.kit())
            .on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Composer(ComposerMsg::RemoveAttachment(index)), tx);
            })
            .build(&t.k)
            .key("remove"),
    )
}

/// Mounts the full-size `rune_kit::Preview` lightbox overlay when an image
/// attachment thumbnail is open.
pub fn lightbox(s: &Desktop, reduced: bool, t: &Theme, layer: &mut Vec<El>) {
    let Some(index) = s.model.composer.lightbox else {
        return;
    };
    let Some(attachment) = s.model.composer.draft.attachments.get(index) else {
        return;
    };
    let (natural, picture) = s
        .attachment_image(index)
        .filter(|img| img.is_ready() && img.error().is_none())
        .map_or_else(
            || {
                (
                    [320.0, 240.0],
                    col()
                        .key(format!("lightbox-fallback-{index}"))
                        .center()
                        .gap(S6)
                        .fill(t.surface_raised)
                        .border(HAIRLINE, t.border_overlay)
                        .child(icon(&ui::EYE, ICON_SMALL + S8).color(t.muted))
                        .child(text(&attachment.name).font(t.ui).color(t.text)),
                )
            },
            |image| {
                (
                    image_dims(image.size()),
                    el().key(format!("lightbox-img-{index}"))
                        .pixels(image.pixels()),
                )
            },
        );
    let window = [s.model.panels.window_width, s.model.panels.window_height];
    layer.push(
        el().key("lightbox-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .fill(t.scrim)
            .transition(motion::scrim()),
    );
    let preview = Preview::new(
        |s: &mut Desktop| &mut s.widgets.image_zoom,
        natural,
        picture,
    )
    .icons(&ui::MINUS, &ui::PLUS)
    .on_close(|s: &mut Desktop, tx| {
        s.dispatch(Msg::Composer(ComposerMsg::OpenLightbox(None)), tx);
    })
    .build(&s.widgets.image_zoom, &t.overlay, window, reduced);
    layer.push(preview);
}

#[expect(
    clippy::cast_precision_loss,
    reason = "decoded image dimensions are small enough for f32 lightbox sizing"
)]
fn image_dims(size: Option<[u32; 2]>) -> [f32; 2] {
    size.map_or([320.0, 240.0], |[w, h]| [w as f32, h as f32])
}

fn image_decode_error(s: &Desktop, index: usize) -> Option<crate::model::AttachmentStatus> {
    let error = s.attachment_image(index)?.error()?;
    Some(crate::model::AttachmentStatus::Error(error.to_string()))
}

/// A retry badge over a thumbnail whose decoder reported an error.
fn retry_thumbnail(index: usize, message: &str, t: &Theme) -> El {
    row()
        .key("retry")
        .role("button")
        .label("Retry attachment")
        .help(format!("Retry ({message})"))
        .pin(Some(S4), None, None, Some(S4))
        .size(MEDIA_CLOSE, MEDIA_CLOSE)
        .center()
        .radius(RADIUS_PILL)
        .fill(t.canvas.alpha(0.85))
        .border(HAIRLINE, t.danger)
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx| {
            crate::attachments::retry_draft_image(s, index);
            tx.wake_after(std::time::Duration::from_millis(32));
        })
        .child(icon(&ui::ROTATE_CW, 11.0).color(t.danger).a11y_hidden())
}
