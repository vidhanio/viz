//! OS platform effects: file-dialog attachment loading, base64 encoding, and
//! desktop notifications.

use std::{
    fs::{self, File},
    io::{self, Read as _},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rune::AppHandle;
use viz_client::protocol::{Attachment, ThreadId};

use super::{AttachmentSource, LoadedAttachments};
use crate::{
    app::Desktop,
    model::AttachmentTarget,
    update::{Msg, ServerMsg},
};

/// Maximum single-file size before base64 encoding.
const FILE_LIMIT: usize = viz_client::protocol::limits::ATTACHMENT_TOTAL_BYTES;
/// Maximum total base64 bytes for attachments matching the server's prompt
/// budget.
pub const ENCODED_LIMIT: usize = viz_client::protocol::limits::ATTACHMENT_ENCODED_LIMIT;

/// Loads attachments synchronously from `source` (intended for the blocking
/// thread pool).
#[must_use]
pub fn load_attachments(source: AttachmentSource) -> LoadedAttachments {
    let paths = match source {
        AttachmentSource::Picker => rfd::FileDialog::new()
            .set_title("Attach files")
            .pick_files()
            .unwrap_or_default(),
        AttachmentSource::Files(paths) => paths,
        AttachmentSource::ClipboardPng(png) => {
            if png.len() > FILE_LIMIT {
                return LoadedAttachments {
                    errors: vec!["Clipboard image exceeds 20 MiB".into()],
                    ..LoadedAttachments::default()
                };
            }
            return LoadedAttachments {
                attachments: vec![Attachment {
                    name: "Clipboard.png".into(),
                    mime: "image/png".into(),
                    data: STANDARD.encode(png),
                }],
                errors: Vec::new(),
            };
        }
    };
    let mut loaded = LoadedAttachments::default();
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

/// Spawns attachment loading on Tokio's blocking pool and feeds the result back
/// as [`ServerMsg::AttachmentsLoaded`].
pub fn spawn_attach(
    ui: &AppHandle<Desktop>,
    scope: u64,
    target: AttachmentTarget,
    source: AttachmentSource,
) {
    let ui = ui.clone();
    tokio::task::spawn_blocking(move || {
        let loaded = load_attachments(source);
        ui.update(move |s, tx| {
            s.dispatch(
                Msg::Server(ServerMsg::AttachmentsLoaded {
                    scope,
                    target,
                    loaded,
                }),
                tx,
            );
        });
    });
}

/// Notification id for a thread so newer updates replace older ones in the
/// system notification center and selecting the thread clears it.
pub fn thread_notification_id(thread: ThreadId) -> String {
    format!("thread-{thread}")
}

/// Registers the app's `rune-notify` identity and click handler so clicking a
/// delivered notification brings viz forward and opens that thread.
pub fn install_notifications(ui: AppHandle<Desktop>) {
    let identity = rune_notify::Identity::new("io.vidhan.viz", "viz");
    _ = rune_notify::register(&identity);
    rune_notify::on_click(move |click| {
        let thread = click
            .get("thread")
            .and_then(|value| value.parse::<u64>().ok())
            .map(ThreadId);
        ui.update(move |s, tx| {
            rune_notify::bring_to_front();
            if let Some(id) = thread {
                s.dispatch(Msg::Selection(crate::update::SelectionMsg::Select(id)), tx);
            }
        });
    });
}

/// Removes any delivered notification for `thread` from the system center.
pub fn clear_thread_notification(thread: ThreadId) {
    let id = thread_notification_id(thread);
    rune_notify::remove(&[id.as_str()]);
}

/// Shows a desktop notification via `rune-notify` (macOS
/// `UNUserNotificationCenter` and Windows toasts), falling back to freedesktop
/// D-Bus on Linux where `rune-notify` is unavailable.
pub fn spawn_notify(thread: ThreadId, title: &str, body: &'static str) {
    if rune_notify::app_focused() {
        return;
    }
    let note = rune_notify::Notification::new(thread_notification_id(thread), title)
        .body(body)
        .info("thread", thread.to_string());
    if rune_notify::post(&note) {
        return;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut notification = notify_rust::Notification::new();
        notification.appname("viz").summary(title).body(body);
        tokio::spawn(async move {
            _ = notification.show_async().await;
        });
    }
}

/// Read a user-selected icon file on the blocking pool.
pub(super) fn pick_project_icon() -> io::Result<Option<viz_client::protocol::ProjectIconChange>> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Choose project icon")
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "gif", "webp", "ico", "svg"],
        )
        .pick_file()
    else {
        return Ok(None);
    };
    read_file(&path)
        .map(super::projects::uploaded_icon)
        .map(Some)
}
