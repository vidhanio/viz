//! Project icon discovery and root-confined file reads.

use std::{
    io::{self, Read},
    path::{Component, Path},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use tokio_util::sync::CancellationToken;

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
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::Invalid("relative icon path"));
    }
    let (root, relative) = (root.to_owned(), relative.to_owned());
    blocking(cancel, move |cancel| {
        let dir = Dir::open_ambient_dir(root, ambient_authority())?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = match dir.open_with(relative, &options) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Invalid("regular icon file"));
        }
        if metadata.len() > ICON_LIMIT {
            return Err(Error::Invalid("icon larger than 1 MiB"));
        }
        let mut bytes = Vec::new();
        file.take(ICON_LIMIT + 1).read_to_end(&mut bytes)?;
        check(cancel)?;
        if bytes.len() as u64 > ICON_LIMIT {
            return Err(Error::Invalid("icon larger than 1 MiB"));
        }
        Ok(Some(bytes))
    })
    .await
}
