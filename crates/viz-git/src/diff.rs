//! Size-bounded structured patches from libgit2.

use std::path::PathBuf;

use git2::{Delta, DiffFindOptions, DiffOptions, FileMode, Patch};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{Error, Repository, check};

const MAX_FILE_BYTES: i64 = 1024 * 1024;
const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 20_000;
const MAX_FILES: usize = 1_000;

/// Which two states to compare. Working tree and base diffs include untracked
/// files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTarget<'a> {
    WorkingTree,
    Staged,
    Against(&'a str),
    Commit(&'a str),
    Uncommitted,
}

/// Opaque fingerprint for a diff response.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RevisionId([u8; 32]);

impl std::fmt::Display for RevisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    Conflicted,
    Untracked,
    Other,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LineStats {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub revision: RevisionId,
    pub files: Vec<DiffFile>,
    /// Aggregate for the returned files with available text statistics.
    pub stats: LineStats,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: Option<PathBuf>,
    pub new_path: Option<PathBuf>,
    pub kind: FileKind,
    pub binary: bool,
    pub truncated: bool,
    pub stats: Option<LineStats>,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: Vec<u8>,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub origin: char,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: Vec<u8>,
}

impl Repository {
    /// Read a structured patch for the requested comparison.
    ///
    /// # Errors
    /// Returns an error if a revision cannot be resolved or Git cannot build
    /// the diff.
    pub async fn diff(
        &self,
        target: DiffTarget<'_>,
        cancel: &CancellationToken,
    ) -> Result<Diff, Error> {
        let revision = match target {
            DiffTarget::Against(revision) | DiffTarget::Commit(revision) => {
                Some(revision.to_owned())
            }
            _ => None,
        };
        let staged = target == DiffTarget::Staged;
        let committed = matches!(target, DiffTarget::Commit(_));
        let combined = target == DiffTarget::Uncommitted;
        self.read(cancel, move |repo, cancel| {
            let mut opts = options();
            let workdir = !staged && !committed;
            opts.include_untracked(workdir)
                .recurse_untracked_dirs(workdir)
                .show_untracked_content(workdir);
            let diff = if let Some(revision) = revision {
                let commit = repo.revparse_single(&revision)?.peel_to_commit()?;
                let tree = commit.tree()?;
                if committed {
                    let parent = if commit.parent_count() == 0 {
                        None
                    } else {
                        Some(commit.parent(0)?.tree()?)
                    };
                    repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut opts))?
                } else {
                    repo.diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))?
                }
            } else if staged || combined {
                let head = if crate::repository::head_unborn(repo)? {
                    None
                } else {
                    Some(repo.head()?.peel_to_tree()?)
                };
                if staged {
                    repo.diff_tree_to_index(head.as_ref(), None, Some(&mut opts))?
                } else {
                    repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut opts))?
                }
            } else {
                repo.diff_index_to_workdir(None, Some(&mut opts))?
            };
            collect(repo, diff, workdir, cancel)
        })
        .await
    }
}

pub fn options() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.max_size(MAX_FILE_BYTES).include_typechange(true);
    opts
}

pub fn collect(
    repo: &git2::Repository,
    mut diff: git2::Diff<'_>,
    worktree: bool,
    cancel: &CancellationToken,
) -> Result<Diff, Error> {
    check(cancel)?;
    diff.find_similar(Some(
        DiffFindOptions::new()
            .renames(true)
            .for_untracked(worktree)
            .rename_limit(200),
    ))?;
    let mut hash = Sha256::new();
    hash.update([u8::from(worktree)]);
    let mut stats = LineStats::default();
    let mut files = Vec::new();
    let mut budget = Budget {
        bytes: MAX_PATCH_BYTES,
        lines: MAX_LINES,
    };
    let mut truncated = diff.deltas().len() > MAX_FILES;
    let odb = repo.odb()?;
    for (index, delta) in diff.deltas().enumerate() {
        check(cancel)?;
        fingerprint(repo, &mut hash, &delta, worktree);
        if index >= MAX_FILES {
            continue;
        }
        let kind = match delta.status() {
            Delta::Added => FileKind::Added,
            Delta::Deleted => FileKind::Deleted,
            Delta::Modified => FileKind::Modified,
            Delta::Renamed => FileKind::Renamed,
            Delta::Copied => FileKind::Copied,
            Delta::Typechange => FileKind::TypeChanged,
            Delta::Conflicted => FileKind::Conflicted,
            Delta::Untracked => FileKind::Untracked,
            _ => FileKind::Other,
        };
        let mut file = DiffFile {
            old_path: delta
                .old_file()
                .exists()
                .then(|| delta.old_file().path().map(PathBuf::from))
                .flatten(),
            new_path: delta
                .new_file()
                .exists()
                .then(|| delta.new_file().path().map(PathBuf::from))
                .flatten(),
            kind,
            binary: false,
            truncated: [delta.old_file(), delta.new_file()]
                .iter()
                .any(|file| oversized(&odb, file)),
            stats: None,
            hunks: Vec::new(),
        };
        if !file.truncated {
            let patch = Patch::from_diff(&diff, index)?;
            if let Some(patch) = patch {
                let (_, added, removed) = patch.line_stats()?;
                file.stats = Some(LineStats { added, removed });
                stats.added += added;
                stats.removed += removed;
                file.hunks =
                    patch_hunks(&patch, &mut budget, &mut hash, &mut file.truncated, cancel)?;
            }
            if let Some(delta) = diff.get_delta(index) {
                file.binary = delta.old_file().is_binary() || delta.new_file().is_binary();
            }
        }
        truncated |= file.truncated;
        files.push(file);
    }
    Ok(Diff {
        revision: RevisionId(hash.finalize().into()),
        files,
        stats,
        truncated,
    })
}

fn oversized(odb: &git2::Odb<'_>, file: &git2::DiffFile<'_>) -> bool {
    let max = MAX_FILE_BYTES.unsigned_abs();
    file.size() > max
        || (file.size() == 0
            && !file.id().is_zero()
            && file.mode() != FileMode::Commit
            && odb
                .read_header(file.id())
                .is_ok_and(|(size, _)| size as u64 > max))
}

fn fingerprint(
    repo: &git2::Repository,
    hash: &mut Sha256,
    delta: &git2::DiffDelta<'_>,
    worktree: bool,
) {
    hash.update(format!("{:?}", delta.status()));
    for file in [delta.old_file(), delta.new_file()] {
        let path = file.path_bytes().unwrap_or_default();
        hash.update(path.len().to_le_bytes());
        hash.update(path);
        hash.update(file.id().as_bytes());
        hash.update(format!("{:?}", file.mode()));
        hash.update(file.size().to_le_bytes());
        if worktree
            && file.id().is_zero()
            && file.mode() != FileMode::Commit
            && let Some(path) = repo
                .workdir()
                .zip(file.path())
                .map(|(cwd, path)| cwd.join(path))
            && let Ok(meta) = path.symlink_metadata()
        {
            hash.update(format!("{:?}", meta.modified()));
            hash.update(meta.len().to_le_bytes());
        }
    }
}

struct Budget {
    bytes: usize,
    lines: usize,
}

fn patch_hunks(
    patch: &Patch<'_>,
    budget: &mut Budget,
    hash: &mut Sha256,
    truncated: &mut bool,
    cancel: &CancellationToken,
) -> Result<Vec<Hunk>, Error> {
    let mut hunks = Vec::new();
    for index in 0..patch.num_hunks() {
        check(cancel)?;
        let (hunk, count) = patch.hunk(index)?;
        hash.update(hunk.header());
        let mut current = Hunk {
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            header: hunk.header().to_vec(),
            lines: Vec::new(),
        };
        let fits = budget.lines > 0 && budget.bytes >= current.header.len();
        if fits {
            budget.bytes -= current.header.len();
        }
        for line_index in 0..count {
            check(cancel)?;
            let line = patch.line_in_hunk(index, line_index)?;
            hash.update(u32::from(line.origin()).to_le_bytes());
            hash.update(line.content());
            if fits && !*truncated && budget.lines > 0 && budget.bytes >= line.content().len() {
                budget.lines -= 1;
                budget.bytes -= line.content().len();
                current.lines.push(DiffLine {
                    origin: line.origin(),
                    old_line: line.old_lineno(),
                    new_line: line.new_lineno(),
                    content: line.content().to_vec(),
                });
            } else {
                *truncated = true;
            }
        }
        if fits && !current.lines.is_empty() {
            hunks.push(current);
        }
    }
    Ok(hunks)
}
