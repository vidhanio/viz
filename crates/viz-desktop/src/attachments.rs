//! Attachment trigger button, paste/drop input adapters, byte formatting, and
//! image thumbnail / draft pill helpers for [`Desktop`].

use std::{path::PathBuf, sync::Arc};

use rune::{ClipboardContent, El, ImageCache, TextEdit, Tx, image::Image};
use rune_kit::IconButtonVariant;

use crate::{
    app::Desktop,
    effects::AttachmentSource,
    model::{DraftAttachment, DraftPill},
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
    rune_kit::IconButton::new(&ui::PAPERCLIP)
        .help(title)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(Size::Compact.kit())
        .disabled(!can_attach)
        .on_press_tx(choose)
        .build(&t.k)
        .key("attach")
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

/// Image handles follow the active draft. Rune deduplicates and releases
/// decoded content as the handles are acquired and dropped.
#[derive(Default)]
pub struct DraftImages {
    source: std::sync::Weak<Vec<DraftAttachment>>,
    handles: Vec<Option<Image>>,
}

impl DraftImages {
    pub fn sync(
        &mut self,
        attachments: &Arc<Vec<DraftAttachment>>,
        decoder: &mut Option<ImageCache>,
    ) {
        let source = Arc::downgrade(attachments);
        if source.ptr_eq(&self.source) {
            return;
        }
        self.handles = attachments
            .iter()
            .map(|attachment| decode(attachment, decoder))
            .collect();
        self.source = source;
    }

    pub fn get(&self, index: usize) -> Option<&Image> {
        self.handles.get(index)?.as_ref()
    }
}

fn decode(attachment: &DraftAttachment, decoder: &mut Option<ImageCache>) -> Option<Image> {
    if !attachment.mime.starts_with("image/") || attachment.bytes.is_empty() {
        return None;
    }
    let decoder = decoder.get_or_insert_with(|| ImageCache::new(Arc::new(|| {})));
    Some(decoder.load_mime(attachment.bytes.clone(), &attachment.mime, 256.0, 2.0))
}

/// Drops a failed thumbnail and asks the decoder to try again.
pub fn retry_draft_image(s: &mut Desktop, index: usize) {
    if let Some(attachment) = s.model.composer.draft.attachments.get(index)
        && let Some(handle) = s.widgets.attachment_images.handles.get_mut(index)
    {
        *handle = None;
        *handle = decode(attachment, &mut s.widgets.images);
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
        edit.insert_pill(pill.reference.mark(), &text[pill.range.clone()]);
        pos = pill.range.end;
    }
    if pos < text.len() {
        edit.insert(&text[pos..]);
    }
    edit
}
