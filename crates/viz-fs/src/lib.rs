//! Root-confined, nonblocking filesystem access shared across `viz-server`,
//! `viz-acp`, and `viz-git`.
//!
//! Every operation resolves a validated [`RelPath`] inside a
//! [`cap_std::fs::Dir`] capability root, opens files with `O_NONBLOCK |
//! O_NOCTTY`, and verifies the opened descriptor is a regular file via `fstat`
//! before reading or writing. This prevents path traversal, symlink escapes
//! outside the root, and FIFO or device-node blocking TOCTOU bugs.

use std::{
    ffi::OsStr,
    fmt,
    io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};

/// Monotonic counter for unique sibling temporary filenames during atomic
/// writes.
static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Errors returned by confined filesystem operations.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// Path was empty, absolute, contained `.`/`..`, or included NUL bytes.
    #[error("invalid relative path: {0}")]
    InvalidPath(&'static str),
    /// Opened descriptor was not a regular file (e.g., directory, FIFO, or
    /// device).
    #[error("path is not a regular file")]
    NotRegularFile,
    /// A symlink was encountered when symlink following was disallowed.
    #[error("symlinks are not allowed for this operation")]
    Symlink,
    /// File exceeded the caller's byte limit.
    #[error("file exceeds {limit}-byte limit")]
    TooLarge {
        /// Maximum permitted byte length.
        limit: u64,
    },
    /// File bytes were not valid UTF-8 text.
    #[error("file is not valid UTF-8 text")]
    InvalidUtf8,
    /// Underlying operating-system I/O error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A validated non-empty relative path whose components are all
/// [`Component::Normal`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelPath(PathBuf);

impl RelPath {
    /// Validates a UTF-8 relative path string.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InvalidPath`] if `raw` is empty, absolute, contains
    /// NUL bytes, or includes any `.`, `..`, or root/prefix component.
    pub fn parse(raw: &str) -> Result<Self, FsError> {
        if raw.is_empty() {
            return Err(FsError::InvalidPath("path is empty"));
        }
        if raw.as_bytes().contains(&0) {
            return Err(FsError::InvalidPath("path contains NUL byte"));
        }
        Self::from_path(Path::new(raw))
    }

    /// Validates an OS [`Path`] as a non-empty normal-component relative path.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InvalidPath`] if `path` is empty or contains any
    /// non-normal component.
    pub fn from_path(path: &Path) -> Result<Self, FsError> {
        if path.as_os_str().is_empty() {
            return Err(FsError::InvalidPath("path is empty"));
        }
        let mut normalized = PathBuf::new();
        let mut count = 0usize;
        for component in path.components() {
            match component {
                Component::Normal(part) => {
                    if part.as_encoded_bytes().contains(&0) {
                        return Err(FsError::InvalidPath("path contains NUL byte"));
                    }
                    normalized.push(part);
                    count += 1;
                }
                Component::CurDir => {
                    return Err(FsError::InvalidPath("'.' components are not allowed"));
                }
                Component::ParentDir => {
                    return Err(FsError::InvalidPath("'..' components are not allowed"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(FsError::InvalidPath("absolute paths are not allowed"));
                }
            }
        }
        if count == 0 {
            return Err(FsError::InvalidPath("path is empty"));
        }
        Ok(Self(normalized))
    }

    /// Borrow the validated relative path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Return the path as UTF-8 when valid.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        self.0.to_str()
    }

    fn split_parent_leaf(&self) -> (Option<&Path>, &OsStr) {
        let leaf = self
            .0
            .file_name()
            .expect("RelPath always contains at least one normal component");
        let parent = self.0.parent().filter(|p| !p.as_os_str().is_empty());
        (parent, leaf)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

/// Capability-confined root directory backed by [`cap_std::fs::Dir`].
pub struct Root {
    dir: Dir,
}

impl fmt::Debug for Root {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Root").finish_non_exhaustive()
    }
}

impl Root {
    /// Opens an existing directory as a confined filesystem root.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if `path` cannot be opened as a directory.
    pub fn open(path: &Path) -> Result<Self, FsError> {
        let dir = Dir::open_ambient_dir(path, ambient_authority())?;
        Ok(Self { dir })
    }

    /// Opens an existing regular file confined within this root using
    /// `O_NONBLOCK | O_NOCTTY` and verifies `fstat` reports a regular file.
    ///
    /// Symlinks that resolve within the root are permitted by `cap-std`;
    /// symlinks escaping the root are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NotRegularFile`] for FIFOs, directories, sockets, or
    /// device nodes, or [`FsError::Io`] on open/confinement failure.
    pub fn open_regular(&self, rel: &RelPath) -> Result<RegularFile, FsError> {
        let mut options = OpenOptions::new();
        options.read(true);
        apply_nonblock(&mut options, false);
        let file = self.dir.open_with(rel.as_path(), &options)?;
        RegularFile::from_opened(file)
    }

    /// Opens a regular file inside this root without following symlinks at any
    /// path component.
    ///
    /// When `write` is `true`, creates the file if missing without truncating
    /// until `fstat` verifies it is a regular file.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Symlink`] if any component is a symlink,
    /// [`FsError::NotRegularFile`] if the opened target is not a regular file,
    /// or [`FsError::Io`] on I/O error.
    pub fn open_regular_nofollow(
        &self,
        rel: &RelPath,
        write: bool,
    ) -> Result<RegularFile, FsError> {
        let (parent, leaf) = rel.split_parent_leaf();
        let parent_dir = match parent {
            Some(parent_rel) => self.open_dir_nofollow(parent_rel)?,
            None => self.dir.try_clone()?,
        };
        let mut options = OpenOptions::new();
        if write {
            options.write(true).create(true);
        } else {
            options.read(true);
        }
        apply_nonblock(&mut options, true);
        let file = parent_dir
            .open_with(leaf, &options)
            .map_err(map_nofollow_err)?;
        RegularFile::from_opened(file)
    }

    /// Atomically writes `bytes` to `rel` inside this root using a unique
    /// sibling temporary file, `sync_all`, and directory-confined rename.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if creating parent directories, writing,
    /// syncing, or renaming fails.
    pub fn write_atomic(&self, rel: &RelPath, bytes: &[u8]) -> Result<(), FsError> {
        self.write_atomic_inner(rel, bytes, false)
    }

    /// Atomically writes `bytes` to `rel` inside this root with owner-only
    /// (`0600`) permissions on Unix.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if creating parent directories, writing,
    /// syncing, or renaming fails.
    pub fn write_atomic_private(&self, rel: &RelPath, bytes: &[u8]) -> Result<(), FsError> {
        self.write_atomic_inner(rel, bytes, true)
    }

    /// Returns whether `rel` exists within this root without following a
    /// trailing symlink.
    #[must_use]
    pub fn exists(&self, rel: &RelPath) -> bool {
        self.dir.symlink_metadata(rel.as_path()).is_ok()
    }

    /// Removes the file at `rel` confined within this root.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if unlinking fails.
    pub fn remove_file(&self, rel: &RelPath) -> Result<(), FsError> {
        self.dir.remove_file(rel.as_path())?;
        Ok(())
    }

    fn write_atomic_inner(
        &self,
        rel: &RelPath,
        bytes: &[u8],
        private: bool,
    ) -> Result<(), FsError> {
        let (parent, leaf) = rel.split_parent_leaf();
        let parent_dir = match parent {
            Some(parent_rel) => {
                self.dir.create_dir_all(parent_rel)?;
                self.dir.open_dir(parent_rel)?
            }
            None => self.dir.try_clone()?,
        };
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let tmp_name = format!(".viz-tmp-{pid}-{seq}");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        apply_nonblock(&mut options, true);
        if private {
            apply_mode(&mut options, 0o600);
        }
        let mut file = parent_dir.open_with(&tmp_name, &options)?;
        let write_result = (|| -> io::Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        drop(file);
        if let Err(error) = write_result {
            let _ = parent_dir.remove_file(&tmp_name);
            return Err(error.into());
        }
        if let Err(error) = parent_dir.rename(&tmp_name, &parent_dir, leaf) {
            let _ = parent_dir.remove_file(&tmp_name);
            return Err(error.into());
        }
        Ok(())
    }

    fn open_dir_nofollow(&self, rel: &Path) -> Result<Dir, FsError> {
        let mut current = self.dir.try_clone()?;
        for component in rel.components() {
            let Component::Normal(part) = component else {
                return Err(FsError::InvalidPath("expected normal directory component"));
            };
            let meta = current.symlink_metadata(part)?;
            if meta.file_type().is_symlink() {
                return Err(FsError::Symlink);
            }
            current = current.open_dir(part)?;
        }
        Ok(current)
    }
}

/// An opened regular file verified by `fstat`.
pub struct RegularFile {
    file: File,
    size: u64,
}

impl fmt::Debug for RegularFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegularFile")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Result of a bounded byte read from a [`RegularFile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedRead {
    /// Bytes read, at most the caller's byte limit.
    pub bytes: Vec<u8>,
    /// Whether additional bytes remained after the limit.
    pub truncated: bool,
    /// File length reported by `fstat` at open time.
    pub size: u64,
}

/// Parameters for [`RegularFile::read_line_window`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineWindowSpec {
    /// 1-based starting line number (`>= 1`).
    pub start: u32,
    /// Maximum number of lines to return (`>= 1`).
    pub count: u32,
}

/// Result of reading a bounded line window from a UTF-8 text file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextWindow {
    /// UTF-8 text for the returned lines.
    pub text: String,
    /// 1-based line number of the first line in `text`.
    pub start_line: u32,
    /// Number of lines contained in `text`.
    pub line_count: u32,
    /// Total lines in the file when EOF was reached within the scan cap.
    pub total_lines: Option<u32>,
    /// Whether the returned content was capped by window, byte, or scan limits.
    pub truncated: bool,
    /// Whether more lines exist after the returned window.
    pub has_more: bool,
    /// File length reported by `fstat` at open time.
    pub size: u64,
}

impl RegularFile {
    fn from_opened(file: File) -> Result<Self, FsError> {
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(FsError::NotRegularFile);
        }
        Ok(Self {
            size: meta.len(),
            file,
        })
    }

    /// Length of the file in bytes reported by `fstat`.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.size
    }

    /// Whether the file is empty according to `fstat`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Reads up to `limit` bytes, reporting truncation without failing on
    /// oversized files.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] on read failure.
    pub fn read_bounded(self, limit: usize) -> Result<BoundedRead, FsError> {
        let cap = usize::try_from(self.size)
            .unwrap_or(limit)
            .min(limit.saturating_add(1));
        let mut bytes = Vec::with_capacity(cap);
        let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
        self.file.take(take_limit).read_to_end(&mut bytes)?;
        let truncated = bytes.len() > limit;
        if truncated {
            bytes.truncate(limit);
        }
        Ok(BoundedRead {
            bytes,
            truncated,
            size: self.size,
        })
    }

    /// Reads at most `len` bytes starting at `offset`, returning `(bytes,
    /// total_size)`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] on seek or read failure.
    pub fn read_range(mut self, offset: u64, len: usize) -> Result<(Vec<u8>, u64), FsError> {
        let total = self.size;
        if offset >= total || len == 0 {
            return Ok((Vec::new(), total));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        let remaining = usize::try_from(total - offset).unwrap_or(len).min(len);
        let mut bytes = Vec::with_capacity(remaining);
        self.file
            .take(u64::try_from(len).unwrap_or(u64::MAX))
            .read_to_end(&mut bytes)?;
        Ok((bytes, total))
    }

    /// Reads the entire file, failing with [`FsError::TooLarge`] if it exceeds
    /// `limit` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::TooLarge`] if the file size or read stream exceeds
    /// `limit`, or [`FsError::Io`] on read failure.
    pub fn read_exact_bounded(self, limit: u64) -> Result<Vec<u8>, FsError> {
        if self.size > limit {
            return Err(FsError::TooLarge { limit });
        }
        let mut bytes = Vec::new();
        self.file
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(FsError::TooLarge { limit });
        }
        Ok(bytes)
    }

    /// Truncates this verified regular file to zero length and writes `bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Io`] if truncating or writing fails.
    pub fn write_all_truncated(mut self, bytes: &[u8]) -> Result<(), FsError> {
        self.file.set_len(0)?;
        self.file.write_all(bytes)?;
        Ok(())
    }

    /// Reads a bounded 1-based line window from a UTF-8 text file, sniffing for
    /// binary bytes up front and capping both returned bytes and total scanned
    /// bytes.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InvalidUtf8`] if the file contains NUL bytes or
    /// invalid UTF-8 sequences, or [`FsError::Io`] on read failure.
    pub fn read_line_window(
        self,
        window: LineWindowSpec,
        max_bytes: usize,
        max_scan_bytes: usize,
    ) -> Result<TextWindow, FsError> {
        let size = self.size;
        let start_line = window.start.max(1);
        let max_lines = window.count.max(1);
        let end_exclusive = start_line.saturating_add(max_lines);

        let mut reader = BufReader::with_capacity(16 * 1024, self.file);
        let initial = reader.fill_buf()?;
        let initial_truncated = u64::try_from(initial.len()).is_ok_and(|len| size > len);
        if looks_binary(initial, initial_truncated) {
            return Err(FsError::InvalidUtf8);
        }

        let mut out = Vec::new();
        let mut line_buf = Vec::new();
        let mut current_line = 0u32;
        let mut collected_lines = 0u32;
        let mut scanned_bytes = 0usize;
        let mut byte_capped = false;
        let mut scan_capped = false;
        let mut has_more = false;
        let mut eof = false;

        loop {
            line_buf.clear();
            let remaining_scan = max_scan_bytes.saturating_sub(scanned_bytes);
            if remaining_scan == 0 {
                scan_capped = true;
                has_more = true;
                break;
            }
            let read_len = read_line_bounded(&mut reader, &mut line_buf, remaining_scan)?;
            if read_len == 0 {
                eof = true;
                break;
            }
            scanned_bytes = scanned_bytes.saturating_add(read_len);
            let hit_scan_mid_line = read_len == remaining_scan && !line_buf.ends_with(b"\n");
            current_line = current_line.saturating_add(1);

            if current_line < start_line {
                if hit_scan_mid_line {
                    scan_capped = true;
                    has_more = true;
                    break;
                }
                continue;
            }

            if current_line < end_exclusive && !byte_capped {
                if out.len().saturating_add(line_buf.len()) > max_bytes {
                    let take = max_bytes.saturating_sub(out.len());
                    if take > 0 {
                        out.extend_from_slice(&line_buf[..take]);
                        collected_lines = collected_lines.saturating_add(1);
                    }
                    byte_capped = true;
                    has_more = true;
                } else {
                    out.extend_from_slice(&line_buf);
                    collected_lines = collected_lines.saturating_add(1);
                }
            } else {
                has_more = true;
            }

            if hit_scan_mid_line {
                scan_capped = true;
                has_more = true;
                break;
            }
        }

        let truncated = byte_capped || scan_capped || start_line > 1 || has_more;
        let text =
            decode_utf8_prefix(&out, byte_capped || scan_capped).ok_or(FsError::InvalidUtf8)?;
        Ok(TextWindow {
            text,
            start_line,
            line_count: collected_lines,
            total_lines: eof.then_some(current_line),
            truncated,
            has_more,
            size,
        })
    }
}

/// Decodes `bytes` as UTF-8, tolerating an incomplete trailing multi-byte
/// character when `truncated` is `true` while still rejecting invalid UTF-8
/// elsewhere.
#[must_use]
pub fn decode_utf8_prefix(bytes: &[u8], truncated: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(valid) => Some(valid.to_owned()),
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .ok()
                .map(str::to_owned)
        }
        Err(_) => None,
    }
}

/// Sniffs a MIME type from magic bytes first, falling back to `path`'s
/// extension.
#[must_use]
pub fn sniff_mime(path: &Path, prefix: &[u8]) -> String {
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".to_owned();
    }
    if prefix.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg".to_owned();
    }
    if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        return "image/gif".to_owned();
    }
    if prefix.len() >= 12 && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WEBP" {
        return "image/webp".to_owned();
    }
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_owned()
}

fn looks_binary(prefix: &[u8], truncated: bool) -> bool {
    if prefix.contains(&0) {
        return true;
    }
    decode_utf8_prefix(prefix, truncated).is_none()
}

fn read_line_bounded(
    reader: &mut BufReader<File>,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> io::Result<usize> {
    let mut total = 0usize;
    while total < max_bytes {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let budget = max_bytes - total;
        let slice = &available[..available.len().min(budget)];
        if let Some(pos) = slice.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            out.extend_from_slice(&slice[..take]);
            reader.consume(take);
            total += take;
            break;
        }
        let take = slice.len();
        out.extend_from_slice(slice);
        reader.consume(take);
        total += take;
    }
    Ok(total)
}

#[cfg(unix)]
fn apply_nonblock(options: &mut OpenOptions, nofollow: bool) {
    use cap_std::fs::OpenOptionsExt as _;

    let mut flags = libc::O_NONBLOCK | libc::O_NOCTTY;
    if nofollow {
        flags |= libc::O_NOFOLLOW;
    }
    options.custom_flags(flags);
}

#[cfg(not(unix))]
fn apply_nonblock(_options: &mut OpenOptions, _nofollow: bool) {}

#[cfg(unix)]
fn apply_mode(options: &mut OpenOptions, mode: u32) {
    use cap_std::fs::OpenOptionsExt as _;

    options.mode(mode);
}

#[cfg(not(unix))]
fn apply_mode(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(unix)]
fn map_nofollow_err(error: io::Error) -> FsError {
    if error.raw_os_error() == Some(libc::ELOOP) {
        FsError::Symlink
    } else {
        FsError::Io(error)
    }
}

#[cfg(not(unix))]
fn map_nofollow_err(error: io::Error) -> FsError {
    FsError::Io(error)
}
