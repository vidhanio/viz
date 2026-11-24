//! Project icon discovery and root-confined file reads.

use std::{io, path::Path};

use tokio_util::sync::CancellationToken;
use viz_fs::{FsError, RelPath, Root};

use crate::{Error, blocking, check};

const ICON_LIMIT: u64 = 1024 * 1024;

/// Relative icon paths in preference order. Project-specific overrides can be
/// tried first.
#[must_use]
pub const fn icon_candidate_paths() -> &'static [&'static str] {
    &[
        "favicon.svg",
        "favicon.ico",
        "favicon.png",
        "favicon.webp",
        "public/favicon.svg",
        "public/favicon.ico",
        "public/favicon.png",
        "public/favicon.webp",
        "app/favicon.ico",
        "app/favicon.png",
        "app/icon.svg",
        "app/icon.png",
        "app/icon.ico",
        "src/favicon.svg",
        "src/favicon.ico",
        "src/app/favicon.ico",
        "src/app/icon.svg",
        "src/app/icon.png",
        "logo.svg",
        "logo.png",
        "logo.webp",
        "public/logo.svg",
        "public/logo.png",
        "assets/icon.svg",
        "assets/icon.png",
        "assets/logo.svg",
        "assets/logo.png",
        "src-tauri/icons/icon.png",
        "src-tauri/icons/128x128.png",
        "src-tauri/icons/icon.ico",
        "src-tauri/icons/icon.icns",
        ".idea/icon.svg",
        "Resources/AppIcon.icns",
        "AppIcon.icns",
        "web/favicon.png",
    ]
}

/// Read up to 1 MiB from a regular file contained by the root. Missing files
/// return `None`.
///
/// # Errors
/// Returns an error for invalid paths, escapes from the root, oversized files,
/// or I/O failures.
pub async fn read_icon(
    root: &Path,
    relative: &Path,
    cancel: &CancellationToken,
) -> Result<Option<Vec<u8>>, Error> {
    let rel = RelPath::from_path(relative).map_err(|_| Error::Invalid("relative icon path"))?;
    let root = root.to_owned();
    blocking(cancel, move |cancel| {
        let dir = Root::open(&root).map_err(map_fs_error)?;
        let file = match dir.open_regular(&rel) {
            Ok(file) => file,
            Err(FsError::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(map_fs_error(error)),
        };
        let bytes = file.read_exact_bounded(ICON_LIMIT).map_err(map_fs_error)?;
        check(cancel)?;
        Ok(Some(bytes))
    })
    .await
}

fn map_fs_error(error: FsError) -> Error {
    match error {
        FsError::InvalidPath(_) => Error::Invalid("relative icon path"),
        FsError::NotRegularFile => Error::Invalid("regular icon file"),
        FsError::Symlink => Error::Invalid("icon symlink"),
        FsError::TooLarge { .. } => Error::Invalid("icon larger than 1 MiB"),
        FsError::InvalidUtf8 => Error::Invalid("icon encoding"),
        FsError::Io(io) => Error::Io(io),
    }
}
