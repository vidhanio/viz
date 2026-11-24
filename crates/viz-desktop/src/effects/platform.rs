//! OS platform effects: file-dialog attachment loading and desktop
//! notifications.

use std::{
    fs::{self, File},
    io::{self, Read as _},
    path::Path,
};

use rune::AppHandle;
use viz_client::protocol::ThreadId;

use super::{AttachmentSource, LoadedAttachments};
use crate::{
    app::Desktop,
    model::{AttachmentTarget, DraftAttachment},
    update::{Msg, ServerMsg},
};

/// Maximum raw attachment size matching the server's prompt budget.
const FILE_LIMIT: usize = viz_client::protocol::limits::ATTACHMENT_TOTAL_BYTES;
/// Maximum uploaded project icon size matching the server's 1 MiB cap.
const ICON_LIMIT: usize = 1024 * 1024;

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
                attachments: vec![DraftAttachment {
                    id: crate::composer::codec::RefId::new(),
                    name: "Clipboard.png".into(),
                    mime: "image/png".into(),
                    bytes: png,
                }],
                errors: Vec::new(),
            };
        }
    };
    let mut loaded = LoadedAttachments::default();
    let mut total_size = 0usize;
    for path in paths {
        match read_file(&path, FILE_LIMIT, "exceeds 20 MiB") {
            Ok(file) if file.bytes.len() + total_size <= FILE_LIMIT => {
                total_size += file.bytes.len();
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

fn read_file(path: &Path, max_bytes: usize, too_large: &str) -> io::Result<DraftAttachment> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("not a file"));
    }
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    if metadata.len() > limit {
        return Err(io::Error::other(too_large));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::other(too_large));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing file name"))?
        .to_string_lossy();
    Ok(DraftAttachment {
        id: crate::composer::codec::RefId::new(),
        name: name.into_owned(),
        mime: mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string(),
        bytes,
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
pub(super) fn pick_project_icon() -> io::Result<Option<DraftAttachment>> {
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
    read_file(&path, ICON_LIMIT, "exceeds 1 MiB").map(Some)
}

/// Checks that a preview path still points to a regular file in the workspace
/// before passing its absolute path to the system handler.
pub(super) fn open_local_file(directory: &Path, path: &str) -> Result<(), String> {
    let relative = viz_fs::RelPath::parse(path).map_err(|error| error.to_string())?;
    let directory = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = directory.join(relative.as_path());
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if !resolved.starts_with(&directory) || !resolved.is_file() {
        return Err("File is outside the workspace or is no longer a regular file.".into());
    }
    open_with_system(&resolved)
}

#[cfg(unix)]
fn open_with_system(path: &Path) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(program)
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// Both wide strings are NUL-terminated and outlive the shell call.
#[cfg(windows)]
#[allow(unsafe_code)]
fn open_with_system(path: &Path) -> Result<(), String> {
    use std::{os::windows::ffi::OsStrExt as _, ptr::null};

    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let verb: Vec<u16> = "open".encode_utf16().chain([0]).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let status = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    if status as usize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute returned {status:p}"))
    }
}

#[cfg(not(any(unix, windows)))]
fn open_with_system(_: &Path) -> Result<(), String> {
    Err("This platform has no file opener.".into())
}
