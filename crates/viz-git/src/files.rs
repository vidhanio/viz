//! Single-pass ignore-aware file and directory lookup with bounded top-K
//! scoring and zero global caching.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ignore::WalkBuilder;
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use tokio_util::sync::CancellationToken;

use crate::{Error, blocking, check};

const MAX_ENTRIES_PER_ROOT: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    pub path: PathBuf,
    pub kind: PathKind,
    pub score: u32,
    /// Sorted grapheme indices in the display path, for highlighting.
    pub positions: Vec<u32>,
}

type Ranked = (Reverse<u32>, PathBuf, bool, String);

/// Find files and directories relative to a root, ordered by descending score
/// then path.
///
/// Inside a Git checkout, streams `git ls-files -z --cached --others
/// --exclude-standard` directly into a bounded top-K heap without buffering the
/// full repository listing in memory. Outside Git, falls back to a single-pass
/// `ignore::WalkBuilder` scan.
///
/// # Errors
/// Returns an error if walking the root fails or the request is cancelled.
pub async fn find_files(
    root: &Path,
    query: &str,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<FileMatch>, Error> {
    let (root, query) = (root.to_owned(), query.to_owned());
    blocking(cancel, move |cancel| {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let canonical = root.canonicalize()?;
        let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut utf32 = Vec::new();
        let mut selected = BinaryHeap::new();

        if !scan_git_files(
            &canonical,
            &pattern,
            &mut matcher,
            &mut utf32,
            &mut selected,
            limit,
            cancel,
        )? {
            scan_walk(
                &canonical,
                &pattern,
                &mut matcher,
                &mut utf32,
                &mut selected,
                limit,
                cancel,
            )?;
        }

        Ok(selected
            .into_sorted_vec()
            .into_iter()
            .map(|(Reverse(score), path, directory, display)| {
                let mut positions = Vec::new();
                pattern.indices(
                    Utf32Str::new(&display, &mut utf32),
                    &mut matcher,
                    &mut positions,
                );
                positions.sort_unstable();
                positions.dedup();
                FileMatch {
                    path,
                    kind: if directory {
                        PathKind::Directory
                    } else {
                        PathKind::File
                    },
                    score,
                    positions,
                }
            })
            .collect())
    })
    .await
}

fn push_candidate(
    display: &str,
    directory: bool,
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32: &mut Vec<char>,
    selected: &mut BinaryHeap<Ranked>,
    limit: usize,
) {
    if let Some(score) = pattern.score(Utf32Str::new(display, utf32), matcher) {
        selected.push((
            Reverse(score),
            PathBuf::from(display),
            directory,
            display.to_owned(),
        ));
        if selected.len() > limit {
            selected.pop();
        }
    }
}

/// Streams `git ls-files -z` when `root` sits inside a Git repository,
/// returning `Ok(true)` if Git handled the scan or `Ok(false)` to fall back to
/// `ignore::WalkBuilder`.
fn scan_git_files(
    root: &Path,
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32: &mut Vec<char>,
    selected: &mut BinaryHeap<Ranked>,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<bool, Error> {
    if crate::fast_head_branch(root).is_none() {
        return Ok(false);
    }
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(false);
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(false);
    };
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::with_capacity(256);
    let mut seen_dirs = HashSet::new();
    let mut count = 0usize;
    loop {
        if count.is_multiple_of(512) && cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Cancelled);
        }
        buf.clear();
        let read = reader.read_until(0, &mut buf)?;
        if read == 0 {
            break;
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        if buf.is_empty() {
            continue;
        }
        count += 1;
        if count > MAX_ENTRIES_PER_ROOT {
            let _ = child.kill();
            break;
        }
        let Ok(display) = std::str::from_utf8(&buf) else {
            continue;
        };
        push_candidate(display, false, pattern, matcher, utf32, selected, limit);
        let mut cursor = display;
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            if !seen_dirs.insert(parent.to_owned()) {
                break;
            }
            push_candidate(parent, true, pattern, matcher, utf32, selected, limit);
            cursor = parent;
        }
    }
    let status = child.wait()?;
    if count <= MAX_ENTRIES_PER_ROOT && !status.success() {
        selected.clear();
        return Ok(false);
    }
    Ok(true)
}

fn scan_walk(
    root: &Path,
    pattern: &Pattern,
    matcher: &mut Matcher,
    utf32: &mut Vec<char>,
    selected: &mut BinaryHeap<Ranked>,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let mut walk = WalkBuilder::new(root);
    walk.require_git(false)
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    for (count, entry) in walk.build().enumerate() {
        if count.is_multiple_of(256) {
            check(cancel)?;
        }
        if count >= MAX_ENTRIES_PER_ROOT {
            break;
        }
        let entry = entry?;
        if entry.depth() == 0 {
            continue;
        }
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if !(kind.is_dir() || kind.is_file() || kind.is_symlink()) {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| Error::Invalid("walk path"))?;
        let Some(display) = path.to_str() else {
            continue;
        };
        push_candidate(
            display,
            kind.is_dir(),
            pattern,
            matcher,
            utf32,
            selected,
            limit,
        );
    }
    Ok(())
}
