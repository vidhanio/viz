//! Attachment button, composer draft chips, read-only timeline chips, and
//! Rune paste/drop input adapters.

use std::path::PathBuf;

use rune::{ClipboardContent, El, Tx, icon, row, text};
use rune_kit::IconButtonVariant;

use crate::{
    app::Desktop,
    effects::AttachmentSource,
    theme::{CARD_INSET, HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, Size, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// Height of an attachment chip in the composer strip.
const CHIP_HEIGHT: f32 = 26.0;
/// The widest an attachment chip's name gets before truncating.
const CHIP_NAME_MAX: f32 = 160.0;

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
    if !clip.files().is_empty() {
        if !s.model.can_attach() {
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
        if !s.model.can_attach() {
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

/// Paperclip attach button inside the composer toolbar.
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
    ui::icon_button(
        &ui::PAPERCLIP,
        title,
        IconButtonVariant::GhostMuted,
        Size::Default,
    )
    .disabled(!s.model.can_attach())
    .on_press_tx(choose)
    .build(&t.k)
    .key("attach")
}

/// Removable attachment chips displayed above the composer editor.
pub fn draft_chips(s: &Desktop, t: &Theme) -> Option<El> {
    let attachments = &s.model.composer.draft.attachments;
    if attachments.is_empty() {
        return None;
    }
    Some(
        row()
            .key("draft-attachments")
            .w_full()
            .px(CARD_INSET)
            .pt(10.0)
            .gap(S6)
            .flex_wrap()
            .children(attachments.iter().enumerate().map(|(index, attachment)| {
                let bytes = decoded_size(&attachment.data);
                let size_label = (bytes > 0).then(|| compact_bytes(bytes));
                row()
                    .key(format!("attachment-{index}"))
                    .h(CHIP_HEIGHT)
                    .pl(S8)
                    .pr(S4)
                    .gap(S6)
                    .center_y()
                    .radius(RADIUS_CONTROL)
                    .fill(t.chip_fill)
                    .border(HAIRLINE, t.hairline)
                    .child(
                        icon(&ui::FILE_READ, ICON_SMALL)
                            .color(t.muted)
                            .a11y_hidden(),
                    )
                    .child(
                        text(&attachment.name)
                            .font(t.small_medium)
                            .color(t.text)
                            .max_w(CHIP_NAME_MAX)
                            .shrink()
                            .truncate()
                            .help_when_truncated(&attachment.name),
                    )
                    .children(
                        size_label.map(|label| {
                            text(label).font(t.mono_micro).color(t.faint).a11y_hidden()
                        }),
                    )
                    .child(
                        ui::icon_button(
                            &ui::CLOSE,
                            "Remove attachment",
                            IconButtonVariant::GhostMuted,
                            Size::Small,
                        )
                        .on_press(move |s: &mut Desktop| {
                            s.dispatch_local(Msg::Composer(ComposerMsg::RemoveAttachment(index)));
                        })
                        .build(&t.k)
                        .key("remove"),
                    )
            })),
    )
}

/// Approximate decoded byte count of a base64 payload.
fn decoded_size(encoded: &str) -> usize {
    let trimmed = encoded.trim_end_matches('=');
    trimmed.len() * 3 / 4
}

fn compact_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * KIB;
    if bytes < KIB {
        format!("{bytes}B")
    } else if bytes < MIB {
        format!("{}KB", (bytes + KIB / 2) / KIB)
    } else {
        let tenths = (bytes * 10 + MIB / 2) / MIB;
        format!("{}.{}MB", tenths / 10, tenths % 10)
    }
}
