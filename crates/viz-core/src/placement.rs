//! Stable names and checkout choices for workspace placement.

/// The branch prefix of generated names.
const BRANCH_PREFIX: &str = "viz/";
/// The longest generated branch or directory slug.
const SLUG_MAX: usize = 40;

/// The stem of a generated branch for a thread.
#[must_use]
pub fn branch_stem(title: &str) -> String {
    format!("{BRANCH_PREFIX}{}", slug(title, "thread"))
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
