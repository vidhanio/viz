//! Git operations for workspace directories.

use std::path::Path;

use git2::{Branch, Repository, StatusOptions};
use viz_client::protocol::{GitSummary, Project};

pub(super) fn project_summary(mut project: Project) -> Project {
    project.git = summary(Path::new(&project.path));
    project
}

pub(super) fn summary(cwd: &Path) -> Option<GitSummary> {
    let repo = Repository::discover(cwd).ok()?;
    repo.workdir()?;
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .filter(|head| head.is_branch())
        .and_then(|head| head.shorthand().ok())
        .map(str::to_owned)
        .or_else(|| {
            repo.find_reference("HEAD")
                .ok()?
                .symbolic_target()
                .ok()
                .flatten()?
                .strip_prefix("refs/heads/")
                .map(str::to_owned)
        });
    let oid = head.as_ref().and_then(git2::Reference::target);
    let distance = (|| {
        let upstream = Branch::wrap(head?).upstream().ok()?;
        repo.graph_ahead_behind(oid?, upstream.get().target()?).ok()
    })();
    Some(GitSummary {
        branch,
        head: oid.map(|oid| oid.to_string()),
        dirty_files: dirty_files(&repo, false).ok()?,
        ahead: distance.map(|(ahead, _)| ahead),
        behind: distance.map(|(_, behind)| behind),
        linked_worktree: repo.is_worktree(),
    })
}

pub(super) fn dirty_files(repo: &Repository, include_ignored: bool) -> Result<usize, git2::Error> {
    let statuses = repo.statuses(Some(
        StatusOptions::new()
            .include_untracked(true)
            .include_ignored(include_ignored)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true),
    ))?;
    Ok(statuses.len())
}
