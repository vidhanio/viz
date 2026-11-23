//! Reads and writes files inside a granted root without following symlinks.

use std::{
    fs::File,
    io::{self, Read as _, Write as _},
    path::{Component, Path},
};

use rustix::fs::{Mode, OFlags, open, openat};

use crate::schema::ReadTextFileRequest;

const FILE_LIMIT: u64 = 16 * 1024 * 1024;

fn open_file(root: &Path, relative: &Path, flags: OFlags) -> io::Result<File> {
    let directories = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut parent = open(root, directories, Mode::empty())?;
    let mut components = relative.components();
    let Some(Component::Normal(name)) = components.next_back() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a file path",
        ));
    };
    for component in components {
        parent = openat(&parent, component.as_os_str(), directories, Mode::empty())?;
    }
    let file = File::from(openat(
        &parent,
        name,
        flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a regular file",
        ));
    }
    Ok(file)
}

pub(super) fn read_text_file(
    root: &Path,
    path: &Path,
    request: &ReadTextFileRequest,
) -> io::Result<String> {
    let mut text = String::new();
    open_file(root, path, OFlags::RDONLY)?
        .take(FILE_LIMIT + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > FILE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds ACP read limit",
        ));
    }
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
    let mut file = open_file(root, path, OFlags::WRONLY | OFlags::CREATE)?;
    file.set_len(0)?;
    file.write_all(content.as_bytes())
}
