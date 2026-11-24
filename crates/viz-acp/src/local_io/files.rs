//! Reads and writes files inside a granted root without following symlinks.

use std::{io, path::Path};

use viz_fs::{FsError, RelPath, Root};

use crate::schema::ReadTextFileRequest;

const FILE_LIMIT: u64 = 16 * 1024 * 1024;

pub(super) fn read_text_file(
    root: &Path,
    path: &Path,
    request: &ReadTextFileRequest,
) -> io::Result<String> {
    let rel = RelPath::from_path(path).map_err(to_io)?;
    let bytes = Root::open(root)
        .map_err(to_io)?
        .open_regular_nofollow(&rel, false)
        .map_err(to_io)?
        .read_exact_bounded(FILE_LIMIT)
        .map_err(to_io)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8"))?;
    if request.line.is_none() && request.limit.is_none() {
        return Ok(text);
    }
    let line =
        request.line.unwrap_or(1).checked_sub(1).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "line numbers start at one")
        })? as usize;
    Ok(text
        .split_inclusive('\n')
        .skip(line)
        .take(request.limit.map_or(usize::MAX, |limit| limit as usize))
        .collect())
}

pub(super) fn write_text_file(root: &Path, path: &Path, content: &str) -> io::Result<()> {
    let rel = RelPath::from_path(path).map_err(to_io)?;
    Root::open(root)
        .map_err(to_io)?
        .open_regular_nofollow(&rel, true)
        .map_err(to_io)?
        .write_all_truncated(content.as_bytes())
        .map_err(to_io)
}

fn to_io(error: FsError) -> io::Error {
    match error {
        FsError::Io(io) => io,
        FsError::InvalidPath(reason) => io::Error::new(io::ErrorKind::InvalidInput, reason),
        FsError::NotRegularFile => {
            io::Error::new(io::ErrorKind::InvalidInput, "expected a regular file")
        }
        FsError::Symlink => io::Error::new(io::ErrorKind::InvalidInput, "symlinks are not allowed"),
        FsError::TooLarge { .. } => {
            io::Error::new(io::ErrorKind::InvalidData, "file exceeds ACP read limit")
        }
        FsError::InvalidUtf8 => {
            io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8")
        }
    }
}
