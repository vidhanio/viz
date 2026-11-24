//! Ignore-aware file and directory lookup with a short-lived listing per root.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use tokio_util::sync::CancellationToken;

use crate::{Error, blocking, check};

const LISTING_TTL: Duration = Duration::from_secs(30);
const CACHED_ROOTS: usize = 8;
const MAX_ENTRIES_PER_ROOT: usize = 200_000;
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;

static LISTINGS: LazyLock<Mutex<HashMap<PathBuf, Listing>>> = LazyLock::new(Mutex::default);

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

#[derive(Clone)]
struct Listing {
    entries: Arc<[Entry]>,
    bytes: usize,
    scanned_at: Instant,
}

struct Entry {
    path: PathBuf,
    display: String,
    kind: PathKind,
}

/// Find files and directories relative to a root, ordered by descending score
/// then path.
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
        let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
        let entries = listing(&root.canonicalize()?, cancel)?;
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut utf32 = Vec::new();
        let mut selected = BinaryHeap::new();
        for entry in entries.iter() {
            check(cancel)?;
            if let Some(score) =
                pattern.score(Utf32Str::new(&entry.display, &mut utf32), &mut matcher)
            {
                selected.push((
                    Reverse(score),
                    &entry.path,
                    entry.kind == PathKind::Directory,
                    &entry.display,
                ));
                if selected.len() > limit {
                    selected.pop();
                }
            }
        }
        Ok(selected
            .into_sorted_vec()
            .into_iter()
            .map(|(Reverse(score), path, directory, display)| {
                let mut positions = Vec::new();
                pattern.indices(
                    Utf32Str::new(display, &mut utf32),
                    &mut matcher,
                    &mut positions,
                );
                positions.sort_unstable();
                positions.dedup();
                FileMatch {
                    path: path.clone(),
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

fn listing(root: &Path, cancel: &CancellationToken) -> Result<Arc<[Entry]>, Error> {
    let cached = LISTINGS
        .lock()
        .map_err(|_| Error::Invalid("file listing lock"))?
        .get(root)
        .cloned();
    if let Some(cached) = cached
        && cached.scanned_at.elapsed() < LISTING_TTL
    {
        return Ok(cached.entries);
    }
    let mut walk = WalkBuilder::new(root);
    walk.require_git(false)
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    let mut entries = Vec::new();
    let mut bytes = 0usize;
    for entry in walk.build() {
        check(cancel)?;
        if entries.len() >= MAX_ENTRIES_PER_ROOT {
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
        bytes = bytes.saturating_add(display.len() * 2 + std::mem::size_of::<Entry>());
        entries.push(Entry {
            path: path.to_owned(),
            display: display.to_owned(),
            kind: if kind.is_dir() {
                PathKind::Directory
            } else {
                PathKind::File
            },
        });
    }
    let entries = Arc::from(entries);
    if bytes <= MAX_CACHE_BYTES {
        let mut cache = LISTINGS
            .lock()
            .map_err(|_| Error::Invalid("file listing lock"))?;
        cache.remove(root);
        let mut total_bytes: usize = cache.values().map(|listing| listing.bytes).sum();
        while cache.len() >= CACHED_ROOTS || total_bytes.saturating_add(bytes) > MAX_CACHE_BYTES {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, listing)| listing.scanned_at)
                .map(|(root, _)| root.clone())
            else {
                break;
            };
            if let Some(removed) = cache.remove(&oldest) {
                total_bytes = total_bytes.saturating_sub(removed.bytes);
            }
        }
        cache.insert(
            root.to_owned(),
            Listing {
                entries: Arc::clone(&entries),
                bytes,
                scanned_at: Instant::now(),
            },
        );
    }
    Ok(entries)
}
