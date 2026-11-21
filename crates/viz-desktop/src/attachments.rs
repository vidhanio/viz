//! File picker, drops, clipboard images, and attachment chips.

use std::{
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rune::{ClipboardContent, ClipboardImage, El, Tx, icon, row, text};
use rune_kit::IconButtonVariant;
use viz_client::protocol::{Attachment, ThreadId};

use crate::{
    app::Desktop,
    theme::{CARD_INSET, HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S4, S6, S8, Size, Theme},
    ui,
};

/// Height of an attachment chip in the composer strip.
const CHIP_HEIGHT: f32 = 26.0;
/// The widest an attachment chip's name gets before truncating.
const CHIP_NAME_MAX: f32 = 160.0;
/// The widest a read-only attachment chip gets in the timeline.
const CHIP_MAX: f32 = 180.0;

const FILE_LIMIT: usize = 20 * 1024 * 1024;
/// Leave room for JSON metadata and the prompt under the server's 128 MiB frame
/// cap.
pub const ENCODED_LIMIT: usize = 120 * 1024 * 1024;

#[derive(Debug)]
pub enum Source {
    Picker,
    Files(Vec<PathBuf>),
    Clipboard(ClipboardImage),
}

/// The composer that requested a file read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Draft(u64),
    Thread(ThreadId),
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub attachments: Vec<Attachment>,
    pub errors: Vec<String>,
}

impl Source {
    pub fn load(self) -> Loaded {
        let paths = match self {
            Self::Picker => rfd::FileDialog::new()
                .set_title("Attach files")
                .pick_files()
                .unwrap_or_default(),
            Self::Files(paths) => paths,
            Self::Clipboard(image) => {
                if image.png().len() > FILE_LIMIT {
                    return Loaded {
                        errors: vec!["Clipboard image exceeds 20 MiB".into()],
                        ..Loaded::default()
                    };
                }
                return Loaded {
                    attachments: vec![Attachment {
                        name: "Clipboard.png".into(),
                        mime: "image/png".into(),
                        data: STANDARD.encode(image.png()),
                    }],
                    errors: Vec::new(),
                };
            }
        };
        let mut loaded = Loaded::default();
        let mut encoded_size = 0;
        for path in paths {
            match read_file(&path) {
                Ok(file) if file.data.len() + encoded_size <= ENCODED_LIMIT => {
                    encoded_size += file.data.len();
                    loaded.attachments.push(file);
                }
                Ok(file) => loaded.errors.push(format!(
                    "{}: attachments exceed the message limit",
                    file.name
                )),
                Err(error) => loaded.errors.push(format!(
                    "{}: {error}",
                    path.file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                )),
            }
        }
        loaded
    }
}

fn read_file(path: &Path) -> io::Result<Attachment> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("not a file"));
    }
    let limit = u64::try_from(FILE_LIMIT).unwrap_or(u64::MAX);
    if metadata.len() > limit {
        return Err(io::Error::other("exceeds 20 MiB"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(io::Error::other("exceeds 20 MiB"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing file name"))?
        .to_string_lossy();
    Ok(Attachment {
        name: name.into_owned(),
        mime: mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string(),
        data: STANDARD.encode(bytes),
    })
}

pub fn choose(s: &mut Desktop, tx: &mut Tx) {
    s.start_attachments(Source::Picker, tx);
}

pub fn drop_files(s: &mut Desktop, paths: &[PathBuf], tx: &mut Tx) {
    if !paths.is_empty() {
        s.start_attachments(Source::Files(paths.to_vec()), tx);
    }
}

pub fn paste(s: &mut Desktop, clip: &ClipboardContent, tx: &mut Tx) -> bool {
    if !clip.files().is_empty() {
        return s.start_attachments(Source::Files(clip.files().to_vec()), tx);
    }
    if let Some(image) = clip.image() {
        return s.start_attachments(Source::Clipboard(image.clone()), tx);
    }
    false
}

pub fn button(s: &Desktop, t: &Theme) -> El {
    let title = if s
        .state
        .as_ref()
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
    .disabled(!s.can_attach())
    .on_press_tx(choose)
    .build(&t.k)
    .key("attach")
}

pub fn draft_chips(s: &Desktop, t: &Theme) -> Option<El> {
    if s.attachments.is_empty() {
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
            .children(s.attachments.iter().enumerate().map(|(index, attachment)| {
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
                    .continuous_corners()
                    .fill(t.chip_fill)
                    .border(HAIRLINE, t.hairline)
                    .help(&attachment.name)
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
                            .truncate(),
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
                            if index < s.attachments.len() {
                                s.attachments.remove(index);
                            }
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

/// An attachment's name with a file mark, one small control tall.
pub fn chip(name: &str, t: &Theme) -> El {
    row()
        .h(Size::Small.height())
        .px(S8)
        .gap(S4)
        .center_y()
        .max_w(CHIP_MAX)
        .min_w(0.0)
        .shrink()
        .radius(RADIUS_CONTROL)
        .continuous_corners()
        .fill(t.chip_fill)
        .border(HAIRLINE, t.hairline)
        .help(name)
        .child(icon(&ui::FILE_READ, ICON_SMALL).color(t.muted))
        .child(text(name).font(t.small).color(t.text).shrink().truncate())
}
