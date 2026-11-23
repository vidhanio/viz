//! Attachment trigger button, paste/drop input adapters, byte formatting, and
//! image thumbnail / draft pill helpers for [`Desktop`].

use std::{path::PathBuf, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rune::{ClipboardContent, El, ImageCache, TextEdit, Tx, image::Image};
use rune_kit::IconButtonVariant;
use viz_client::protocol::Attachment;

use crate::{
    app::Desktop,
    effects::AttachmentSource,
    model::DraftPill,
    theme::{Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Dispatches a file-picker attachment request.
pub fn choose(s: &mut Desktop, tx: &mut Tx) {
    s.dispatch(
        Msg::Composer(ComposerMsg::StartAttachments(AttachmentSource::Picker)),
        tx,
    );
}

/// Dispatches attachments for paths dropped onto the composer card.
pub fn drop_files(s: &mut Desktop, paths: &[PathBuf], tx: &mut Tx) {
    if !paths.is_empty() {
        s.dispatch(
            Msg::Composer(ComposerMsg::StartAttachments(AttachmentSource::Files(
                paths.to_vec(),
            ))),
            tx,
        );
    }
}

/// Intercepts pasted files or clipboard images and converts them into pure
/// attachment messages.
pub fn paste(s: &mut Desktop, clip: &ClipboardContent, tx: &mut Tx) -> bool {
    let can_attach =
        s.model.can_attach() || (s.model.connected() && s.model.selection.selected.is_none());
    if !clip.files().is_empty() {
        if !can_attach {
            return false;
        }
        s.dispatch(
            Msg::Composer(ComposerMsg::StartAttachments(AttachmentSource::Files(
                clip.files().to_vec(),
            ))),
            tx,
        );
        return true;
    }
    if let Some(image) = clip.image() {
        if !can_attach {
            return false;
        }
        s.dispatch(
            Msg::Composer(ComposerMsg::StartAttachments(
                AttachmentSource::ClipboardPng(image.png().to_vec()),
            )),
            tx,
        );
        return true;
    }
    false
}

/// Paperclip attach button inside the composer toolbar (`28×28` `icon-sm`).
pub fn button(s: &Desktop, t: &Theme) -> El {
    let title = if s
        .model
        .thread_state()
        .is_some_and(|state| state.attachments.images)
    {
        "Attach files or paste an image"
    } else {
        "Attach files"
    };
    let can_attach =
        s.model.can_attach() || (s.model.connected() && s.model.selection.selected.is_none());
    ui::icon_button(
        &ui::PAPERCLIP,
        title,
        IconButtonVariant::GhostMuted,
        Size::Compact,
    )
    .disabled(!can_attach)
    .on_press_tx(choose)
    .build(&t.k)
    .key("attach")
}

/// Approximate decoded byte count of a base64 payload.
#[must_use]
pub fn decoded_size(encoded: &str) -> usize {
    let trimmed = encoded.trim_end_matches('=');
    trimmed.len() * 3 / 4
}

/// Formats byte sizes compactly (`512 B`, `14 KB`, `1.4 MB`).
#[must_use]
pub fn compact_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * KIB;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{} KB", (bytes + KIB / 2) / KIB)
    } else {
        let tenths = (bytes * 10 + MIB / 2) / MIB;
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

/// Stable cache key for a decoded attachment image handle.
#[must_use]
pub fn image_cache_key(attachment: &Attachment) -> String {
    let prefix = &attachment.data[..attachment.data.len().min(32)];
    format!(
        "{}\0{}\0{}\0{prefix}",
        attachment.name,
        attachment.mime,
        attachment.data.len()
    )
}

/// Decodes any uncached image attachments in `s.model.composer.draft` into
/// `s.widgets.attachment_thumbs`.
pub fn decode_draft_images(s: &mut Desktop) {
    for attachment in &s.model.composer.draft.attachments {
        if !attachment.mime.starts_with("image/") || attachment.data.is_empty() {
            continue;
        }
        let key = image_cache_key(attachment);
        if s.widgets.attachment_thumbs.contains_key(&key) {
            continue;
        }
        if let Ok(bytes) = STANDARD.decode(&attachment.data) {
            let cache = s
                .widgets
                .images
                .get_or_insert_with(|| ImageCache::new(Arc::new(|| {})));
            let image = cache.load_mime(bytes, &attachment.mime, 256.0, 2.0);
            s.widgets.attachment_thumbs.insert(key, image);
        }
    }
}

/// Drops a failed thumbnail handle and asks the decoder to try again.
pub fn retry_draft_image(s: &mut Desktop, index: usize) {
    if let Some(attachment) = s.model.composer.draft.attachments.get(index) {
        s.widgets
            .attachment_thumbs
            .remove(&image_cache_key(attachment));
        decode_draft_images(s);
    }
}

/// Rebuilds a [`TextEdit`] with `text` and atomic `Mark::Pill` ranges from
/// `pills`.
#[must_use]
pub fn build_draft_edit(text: &str, pills: &[DraftPill]) -> TextEdit {
    let mut edit = TextEdit::new("");
    let mut sorted: Vec<&DraftPill> = pills
        .iter()
        .filter(|pill| {
            pill.range.start < pill.range.end
                && pill.range.end <= text.len()
                && text.is_char_boundary(pill.range.start)
                && text.is_char_boundary(pill.range.end)
        })
        .collect();
    sorted.sort_by_key(|pill| pill.range.start);
    let mut pos = 0usize;
    for pill in sorted {
        if pill.range.start < pos {
            continue;
        }
        if pill.range.start > pos {
            edit.insert(&text[pos..pill.range.start]);
        }
        edit.insert_pill(&pill.id, &text[pill.range.clone()]);
        pos = pill.range.end;
    }
    if pos < text.len() {
        edit.insert(&text[pos..]);
    }
    edit
}

/// Returns the decoded thumbnail image handle for an attachment, if loaded.
#[must_use]
pub fn cached_image<'a>(s: &'a Desktop, attachment: &Attachment) -> Option<&'a Image> {
    s.widgets
        .attachment_thumbs
        .get(&image_cache_key(attachment))
}

/// Synchronizes the composer [`TextEdit`] text, atomic `Mark::Pill` ranges,
/// and focus flag into `s.model.composer`.
pub fn sync_draft_text(s: &mut Desktop) {
    if s.widgets.draft.text != s.model.composer.draft.text {
        s.model
            .composer
            .draft
            .text
            .clone_from(&s.widgets.draft.text);
    }
    let pills: Vec<DraftPill> = s
        .widgets
        .draft
        .pills()
        .map(|(range, id)| DraftPill {
            label: s.widgets.draft.text[range.clone()].to_owned(),
            range,
            id: id.to_owned(),
        })
        .collect();
    if pills != s.model.composer.draft.pills {
        s.model.composer.draft.pills = pills;
    }
    s.model.composer.draft_focused = s.widgets.draft.focused;
}
