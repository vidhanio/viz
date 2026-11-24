//! Where threads run, and what viz names the checkouts it creates.
//!
//! A thread's project says where it is filed; its [`Checkout`] is where it
//! runs. They normally agree, but filing a thread under another project leaves
//! its checkout alone. A checkout is therefore used as a place in a project
//! only when it lies in the project's repository; otherwise the project's own
//! checkout is.

use viz_protocol::ThreadId;

/// The branch prefix of generated names.
const BRANCH_PREFIX: &str = "viz/";
/// The longest generated branch or directory slug.
const SLUG_MAX: usize = 40;

/// A working directory, and the linked worktree containing it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub cwd: String,
    pub worktree: Option<String>,
}

/// Why a thread cannot be placed, or a checkout removed.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("a worktree requires a project")]
    NoProject,
    #[error("the thread does not have a projectless scratch directory")]
    NotScratch,
    #[error("project name must not be empty")]
    EmptyProjectName,
    #[error("{0} is not a worktree viz created")]
    Unmanaged(String),
    #[error("thread {user} still runs in {path}")]
    InUse { user: ThreadId, path: String },
}

/// A repository as the shell found it on disk: two paths are in one repository
/// when they report the same identity. Linked worktrees share it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository(pub String);

/// A directory and the repository it is in, if any.
#[derive(Debug, Clone, Copy)]
pub struct Site<'a> {
    pub path: &'a str,
    pub repository: Option<&'a Repository>,
}

/// Whether `site` is a place in `project`: in its repository, or below its
/// directory if it has none.
#[must_use]
pub fn within(site: Site<'_>, project: Site<'_>) -> bool {
    match (site.repository, project.repository) {
        (Some(site), Some(project)) => site == project,
        (None | Some(_), None) => std::path::Path::new(site.path).starts_with(project.path),
        (None, Some(_)) => false,
    }
}

/// A thread on disk: the project it is filed under, and where it runs.
#[derive(Debug, Clone, Copy)]
pub struct Placed<'a> {
    pub project: Option<&'a str>,
    pub checkout: &'a Checkout,
    /// The checkout is a place in the project (see [`within`]).
    pub within: bool,
}

impl<'a> Placed<'a> {
    /// The directory a worktree for `project` branches from and mirrors: this
    /// thread's own when it is a place in that project, or else the project's.
    #[must_use]
    pub fn source(self, project: &'a str) -> &'a str {
        if self.within && self.project == Some(project) {
            &self.checkout.cwd
        } else {
            project
        }
    }
}

/// The project a child is filed under: the one it names, or its local
/// parent's.
#[must_use]
pub fn child_project<'a>(target: Option<&'a str>, parent: Option<Placed<'a>>) -> Option<&'a str> {
    target.or_else(|| parent?.project)
}

/// Where a child without a worktree of its own runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place<'a> {
    /// In the parent's checkout.
    Parent(&'a Checkout),
    /// In the project's checkout.
    Project(&'a str),
    /// In the server's default directory.
    Default,
}

/// Place a child.
///
/// A child that names its project runs in that project's checkout. One that
/// names none shares its local parent's, unless that lies outside the project
/// the two are filed under.
#[must_use]
pub const fn child<'a>(target: Option<&'a str>, parent: Option<Placed<'a>>) -> Place<'a> {
    match (target, parent) {
        (Some(project), _) => Place::Project(project),
        (None, None) => Place::Default,
        (None, Some(parent)) => match parent.project {
            Some(project) if !parent.within => Place::Project(project),
            Some(_) | None => Place::Parent(parent.checkout),
        },
    }
}

/// The project a child's own worktree belongs to, and where it branches from.
///
/// # Errors
/// A worktree belongs to a project.
pub fn child_worktree<'a>(
    target: Option<&'a str>,
    parent: Option<Placed<'a>>,
) -> Result<(&'a str, &'a str), Rejection> {
    let project = child_project(target, parent).ok_or(Rejection::NoProject)?;
    let source = parent.map_or(project, |parent| parent.source(project));
    Ok((project, source))
}

/// Whether viz may remove the worktree at `path`: it created it, and no other
/// thread runs there.
///
/// # Errors
/// Names the reason it may not.
pub fn removable(path: &str, managed: bool, user: Option<ThreadId>) -> Result<(), Rejection> {
    if !managed {
        return Err(Rejection::Unmanaged(path.to_owned()));
    }
    user.map_or(Ok(()), |user| {
        Err(Rejection::InUse {
            user,
            path: path.to_owned(),
        })
    })
}

/// The stem of a generated branch for a thread.
#[must_use]
pub fn branch_stem(title: &str) -> String {
    format!("{BRANCH_PREFIX}{}", slug(title, "thread"))
}

/// Pick a collision-free generated branch name from the local ref names.
#[must_use]
pub fn available_branch<'a>(
    title: &str,
    requested: Option<&str>,
    prefix: Option<&str>,
    names: impl IntoIterator<Item = &'a str>,
) -> String {
    if let Some(requested) = requested {
        return requested.to_owned();
    }
    let names = names.into_iter().collect::<std::collections::HashSet<_>>();
    let stem = prefix.map_or_else(
        || branch_stem(title),
        |prefix| format!("{prefix}{}", slug(title, "thread")),
    );
    let last = u32::try_from(names.len())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    (1..=last)
        .map(|suffix| numbered(&stem, suffix))
        .find(|candidate| !names.contains(candidate.as_str()))
        .unwrap_or(stem)
}

/// The repository's directory under the managed worktrees root.
#[must_use]
pub fn repository_dir(name: &str) -> String {
    slug(name, "repo")
}

/// The stem of a managed checkout directory for a branch.
#[must_use]
pub fn worktree_stem(branch: &str) -> String {
    slug(branch, "worktree")
}

/// Number colliding names while keeping the first candidate unsuffixed.
#[must_use]
pub fn numbered(stem: &str, suffix: u32) -> String {
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

/// Name a fresh projectless workspace from its creation date, title, and unique
/// suffix.
#[must_use]
pub fn scratch_name(date: &str, title: &str, suffix: &str) -> String {
    format!("{date}-{}-{suffix}", slug(title, "thread"))
}

/// Decide whether a checkout may be registered in place as a project.
#[derive(Debug)]
pub struct ScratchPromotion;

impl ScratchPromotion {
    /// Return the destination already used by the thread.
    ///
    /// # Errors
    /// Refuses threads that already have a project or do not own a scratch
    /// directory.
    pub fn decide(
        root: &std::path::Path,
        checkout: &Checkout,
        project: Option<&str>,
    ) -> Result<String, Rejection> {
        if project.is_some()
            || checkout.worktree.is_some()
            || std::path::Path::new(&checkout.cwd).parent() != Some(root)
        {
            return Err(Rejection::NotScratch);
        }
        Ok(checkout.cwd.clone())
    }

    /// Choose the explicit name, or use the thread title when available.
    ///
    /// # Errors
    /// Rejects an explicitly blank display name.
    pub fn name(requested: Option<String>, title: String) -> Result<String, Rejection> {
        if let Some(name) = requested {
            if name.trim().is_empty() {
                return Err(Rejection::EmptyProjectName);
            }
            return Ok(name);
        }
        if title.trim().is_empty() {
            Ok("Scratch project".to_owned())
        } else {
            Ok(title)
        }
    }
}
