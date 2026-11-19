//! File picker, drops, clipboard images, and attachment chips.

use std::{
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rune::{ClipboardContent, ClipboardImage, El, Icon, Tx, icon, row, text};
use rune_kit::IconButtonVariant;
use viz_client::protocol::Attachment;

use crate::{app::Desktop, theme::Theme, ui};

const FILE_LIMIT: usize = 20 * 1024 * 1024;
// Leave room for JSON metadata and the prompt under the server's 128 MiB frame
// cap.
pub const ENCODED_LIMIT: usize = 120 * 1024 * 1024;

#[derive(Debug)]
pub enum Source {
    Picker,
    Files(Vec<PathBuf>),
    Clipboard(ClipboardImage),
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
        .conversation
        .as_ref()
        .is_some_and(|state| state.attachments.images)
    {
        "Attach files or paste an image"
    } else {
        "Attach files"
    };
    ui::icon_button(&PAPERCLIP, title, IconButtonVariant::GhostMuted)
        .disabled(!s.can_attach())
        .on_press_tx(choose)
        .build(&t.k)
        .key("attach")
        .size(29.0, 29.0)
}

pub fn draft_chips(s: &Desktop, t: &Theme) -> Option<El> {
    if s.attachments.is_empty() {
        return None;
    }
    Some(
        row()
            .key("draft-attachments")
            .w_full()
            .px(12.0)
            .pt(9.0)
            .gap(5.0)
            .flex_wrap()
            .children(s.attachments.iter().enumerate().map(|(index, attachment)| {
                chip(&attachment.name, t)
                    .key(format!("attachment-{index}"))
                    .child(
                        ui::icon_button(
                            t.k.glyph.close,
                            "Remove attachment",
                            IconButtonVariant::GhostMuted,
                        )
                        .on_press(move |s: &mut Desktop| {
                            if index < s.attachments.len() {
                                s.attachments.remove(index);
                            }
                        })
                        .build(&t.k)
                        .key("remove")
                        .size(18.0, 20.0),
                    )
            })),
    )
}

pub fn chip(name: &str, t: &Theme) -> El {
    row()
        .h(25.0)
        .px(6.0)
        .gap(5.0)
        .center_y()
        .max_w(178.0)
        .min_w(0.0)
        .shrink()
        .radius(t.k.radius.sm)
        .border(1.0, t.hairline)
        .help(name)
        .child(icon(&PAPERCLIP, 12.0).color(t.muted))
        .child(text(name).font(t.small).color(t.text).shrink().truncate())
}

static PAPERCLIP: Icon = Icon {
    name: "viz-paperclip",
    view: 16.0,
    body: r#"<path d="M6 9.75 10.5 5.2a1.45 1.45 0 0 1 2.05 2.05L7.35 12.5a2.65 2.65 0 0 1-3.75-3.75L9.1 3.25" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>"#,
};
