//! Linked Git worktrees for project threads.
//!
//! viz runs the `git` executable rather than linking a Git library, so
//! checkouts honour the user's configuration, hooks, filters and sparse
//! checkouts exactly as their own `git worktree add` would. Reading a
//! checkout's branch only reads its `HEAD` file, so listings spawn nothing.
//!
//! A worktree is viz's own (and may be removed by viz) when it lives under the
//! environment's `worktrees` directory. Branches are never deleted.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio::process::Command;
use viz_client::protocol::GitBranch;

use crate::wire::RpcError;

/// The branch prefix of generated names.
const BRANCH_PREFIX: &str = "viz/";
/// The longest generated branch or directory slug.
const SLUG_MAX: usize = 40;

/// A Git checkout as read from disk.
pub(super) struct Checkout {
    /// The current branch; `None` on a detached HEAD.
    pub(super) branch: Option<String>,
}

/// The checkout containing `path`, if any.
pub(super) fn checkout(path: &Path) -> Option<Checkout> {
    let git_dir = git_dir(path)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    Some(Checkout {
        branch: head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .filter(|name| *name != ".invalid")
            .map(str::to_owned),
    })
}

/// The Git directory of the checkout containing `path`, following a linked
/// worktree's `.git` file.
fn git_dir(path: &Path) -> Option<PathBuf> {
    for dir in path.ancestors() {
        let dot = dir.join(".git");
        let Ok(metadata) = std::fs::metadata(&dot) else {
            continue;
        };
        if metadata.is_dir() {
            return Some(dot);
        }
        let link = std::fs::read_to_string(&dot).ok()?;
        let target = Path::new(link.trim().strip_prefix("gitdir:")?.trim());
        return Some(dir.join(target));
    }
    None
}

/// Where a thread runs: its working directory, and the linked worktree root
/// containing it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placement {
    pub(super) cwd: String,
    pub(super) worktree: Option<String>,
    /// The worktree and branch were created for this placement, so a failed
    /// registration should remove them.
    pub(super) created: Option<String>,
}

/// One entry of `git worktree list`.
pub(super) struct Listed {
    pub(super) path: String,
    pub(super) branch: Option<String>,
    pub(super) main: bool,
}

/// Runs `git -C dir args`, returning stdout; a failure carries Git's message.
async fn git(dir: &Path, args: &[&str]) -> Result<String, RpcError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| RpcError::unavailable(format!("cannot run git: {error}")))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|_| RpcError::invalid("git output is not UTF-8"))
    } else {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.trim();
        Err(RpcError::invalid(if message.is_empty() {
            format!("git {} failed", args.first().unwrap_or(&""))
        } else {
            message
                .strip_prefix("fatal: ")
                .unwrap_or(message)
                .to_owned()
        }))
    }
}

/// Whether `git` succeeds, for checks reported through the exit status.
async fn git_ok(dir: &Path, args: &[&str]) -> Result<bool, RpcError> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map(|status| status.success())
        .map_err(|error| RpcError::unavailable(format!("cannot run git: {error}")))
}

/// The checkout root containing `dir` and `dir`'s path below it, with a
/// trailing slash unless empty.
async fn toplevel(dir: &Path) -> Result<(PathBuf, String), RpcError> {
    let output = git(dir, &["rev-parse", "--show-toplevel", "--show-prefix"]).await?;
    let mut lines = output.lines();
    let root = lines
        .next()
        .filter(|root| !root.is_empty())
        .ok_or_else(|| RpcError::invalid("not a Git checkout"))?;
    Ok((PathBuf::from(root), lines.next().unwrap_or("").to_owned()))
}

/// The worktrees of the repository containing `dir`, main checkout first.
pub(super) async fn list(dir: &Path) -> Result<Vec<Listed>, RpcError> {
    let output = git(dir, &["worktree", "list", "--porcelain", "-z"]).await?;
    let mut worktrees = Vec::new();
    for record in output.split("\0\0").filter(|record| !record.is_empty()) {
        let mut path = None;
        let mut branch = None;
        let mut bare = false;
        for field in record.split('\0') {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(value.to_owned());
            } else if let Some(value) = field.strip_prefix("branch refs/heads/") {
                branch = Some(value.to_owned());
            } else if field == "bare" {
                bare = true;
            }
        }
        if let Some(path) = path
            && !bare
        {
            worktrees.push(Listed {
                path,
                branch,
                main: worktrees.is_empty(),
            });
        }
    }
    Ok(worktrees)
}

/// Local branches, then remote-tracking branches, of the repository
/// containing `dir`.
pub(super) async fn branches(dir: &Path) -> Result<Vec<GitBranch>, RpcError> {
    let output = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(symref)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let (name, symref) = line.split_once('\0')?;
            if !symref.is_empty() {
                return None;
            }
            let (name, remote) = match name.strip_prefix("refs/heads/") {
                Some(local) => (local, false),
                None => (name.strip_prefix("refs/remotes/")?, true),
            };
            Some(GitBranch {
                name: name.to_owned(),
                remote,
            })
        })
        .collect())
}

/// Resolves an existing checkout of `source`'s repository: the main checkout
/// gives no worktree.
pub(super) async fn existing(source: &Path, path: &str) -> Result<Placement, RpcError> {
    let (_, prefix) = toplevel(source).await?;
    let requested = tokio::fs::canonicalize(path)
        .await
        .map_err(|error| RpcError::invalid(format!("cannot read worktree {path}: {error}")))?;
    let found = list(source)
        .await?
        .into_iter()
        .find(|worktree| Path::new(&worktree.path) == requested)
        .ok_or_else(|| {
            RpcError::invalid(format!(
                "{path} is not a worktree of this project's repository"
            ))
        })?;
    Ok(Placement {
        cwd: below(Path::new(&found.path), &prefix)?,
        worktree: (!found.main).then_some(found.path),
        created: None,
    })
}

/// Creates a linked worktree of `source`'s repository under `root`, on a new
/// branch from `base` (default `source`'s HEAD). An unnamed branch gets the
/// `viz/` prefix and a slug of `title`. The branch never tracks its base, so a
/// later push cannot land on the base's upstream.
pub(super) async fn create(
    root: &Path,
    source: &Path,
    base: Option<&str>,
    branch: Option<&str>,
    title: &str,
) -> Result<Placement, RpcError> {
    let (top, prefix) = toplevel(source).await?;
    let base = base.unwrap_or("HEAD");
    let commit = format!("{base}^{{commit}}");
    let oid = git(
        source,
        &["rev-parse", "--verify", "--end-of-options", &commit],
    )
    .await
    .map_err(|_| RpcError::invalid(format!("no commit at {base}")))?;
    let branch = if let Some(branch) = branch {
        let branch = git(source, &["check-ref-format", "--branch", branch])
            .await
            .map_err(|_| RpcError::invalid(format!("invalid branch name {branch:?}")))?
            .trim()
            .to_owned();
        if branch_exists(source, &branch).await? {
            return Err(RpcError::invalid(format!("branch {branch} already exists")));
        }
        branch
    } else {
        let stem = format!("{BRANCH_PREFIX}{}", slug(title, "thread"));
        let mut suffix = 1;
        loop {
            let name = numbered(&stem, suffix);
            if !branch_exists(source, &name).await? {
                break name;
            }
            suffix += 1;
        }
    };
    let repo = top
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "repo".to_owned(), |name| slug(name, "repo"));
    let parent = root.join(repo);
    tokio::fs::create_dir_all(&parent)
        .await
        .map_err(RpcError::internal)?;
    let stem = slug(&branch.replace('/', "-"), "worktree");
    let mut suffix = 1;
    let path = loop {
        let path = parent.join(numbered(&stem, suffix));
        if !tokio::fs::try_exists(&path).await.unwrap_or(true) {
            break path;
        }
        suffix += 1;
    };
    let path_text = path
        .to_str()
        .ok_or_else(|| RpcError::invalid("worktree path is not UTF-8"))?
        .to_owned();
    git(
        source,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &branch,
            "--",
            &path_text,
            oid.trim(),
        ],
    )
    .await?;
    Ok(Placement {
        cwd: below(&path, &prefix)?,
        worktree: Some(path_text.clone()),
        created: Some(branch),
    })
}

async fn branch_exists(dir: &Path, branch: &str) -> Result<bool, RpcError> {
    git_ok(
        dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .await
}

/// Removes the linked worktree at `path`, refusing uncommitted changes and
/// untracked (unignored) files unless `force`. Its branch stays. A worktree
/// already deleted from disk is pruned from `source`'s repository instead.
pub(super) async fn remove(source: &Path, path: &str, force: bool) -> Result<(), RpcError> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        if source == Path::new(path) {
            return Err(RpcError::not_found(format!("no worktree at {path}")));
        }
        return git(source, &["worktree", "prune"]).await.map(drop);
    }
    let dir = Path::new(path);
    if !force && !clean(dir).await? {
        return Err(RpcError::invalid(format!(
            "worktree {path} has uncommitted changes or untracked files"
        )));
    }
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", path]);
    git(dir, &args).await.map(drop)
}

/// Whether the checkout at `dir` has no uncommitted changes or untracked
/// files; ignored files such as build output don't count.
pub(super) async fn clean(dir: &Path) -> Result<bool, RpcError> {
    Ok(git(
        dir,
        &["status", "--porcelain", "-z", "--untracked-files=normal"],
    )
    .await?
    .is_empty())
}

/// Undoes a [`create`] whose registration failed.
pub(super) async fn discard(source: &Path, placement: &Placement) {
    let (Some(path), Some(branch)) = (&placement.worktree, &placement.created) else {
        return;
    };
    if let Err(error) = remove(source, path, true).await {
        tracing::warn!(%path, %error, "rolling back worktree");
        return;
    }
    if let Err(error) = git(source, &["branch", "-D", "--", branch]).await {
        tracing::warn!(%branch, %error, "rolling back worktree branch");
    }
}

/// The directory `prefix` (from `--show-prefix`) below checkout `root`.
fn below(root: &Path, prefix: &str) -> Result<String, RpcError> {
    let prefix = prefix.trim_end_matches('/');
    let dir = if prefix.is_empty() {
        root.to_path_buf()
    } else {
        root.join(prefix)
    };
    dir.into_os_string()
        .into_string()
        .map_err(|_| RpcError::invalid("worktree path is not UTF-8"))
}

fn numbered(stem: &str, suffix: u32) -> String {
    if suffix == 1 {
        stem.to_owned()
    } else {
        format!("{stem}-{suffix}")
    }
}

fn slug(text: &str, fallback: &str) -> String {
    let mut slug = String::new();
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        if slug.len() + word.len() + usize::from(!slug.is_empty()) > SLUG_MAX {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word.to_ascii_lowercase());
    }
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}
