//! Git workspace state and responsive details layout.

use viz_client::protocol::{
    BranchList, GitDiff, GitDiffTarget, GitHubCheck, GitHubReview, GitStatus, ThreadId,
};

use super::Model;

pub const CARD_WIDTH: f32 = 280.0;
pub const CARD_INSET: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Full,
    Compact,
    Essential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    Branch,
    Actions,
    Checks,
    Reviews,
    Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Commit,
    Push,
    PullRequest,
    Restore,
}

#[derive(Debug, Clone)]
pub struct GitState {
    pub watch: Option<(ThreadId, String)>,
    pub status: Option<GitStatus>,
    pub popup: Option<Popup>,
    pub dialog: Option<Dialog>,
    pub query: String,
    pub branch_highlight: Option<usize>,
    pub branches: Option<BranchList>,
    pub default_branch: Option<String>,
    pub new_worktree: bool,
    pub remote_base: bool,
    pub target: GitDiffTarget,
    pub diff: Option<std::sync::Arc<GitDiff>>,
    pub diff_error: Option<String>,
    pub preview: Option<GitDiff>,
    pub checks: Option<Vec<GitHubCheck>>,
    pub reviews: Option<Vec<GitHubReview>>,
    pub excluded: std::collections::BTreeSet<String>,
    pub error: Option<String>,
    pub busy: bool,
    pub serial: u64,
    pub branch_request: u64,
    pub diff_request: u64,
    pub overlay_request: u64,
    pub mutation_request: u64,
}

impl Default for GitState {
    fn default() -> Self {
        Self {
            watch: None,
            status: None,
            popup: None,
            dialog: None,
            query: String::new(),
            branch_highlight: None,
            branches: None,
            default_branch: None,
            new_worktree: false,
            remote_base: false,
            target: GitDiffTarget::Working,
            diff: None,
            diff_error: None,
            preview: None,
            checks: None,
            reviews: None,
            excluded: std::collections::BTreeSet::new(),
            error: None,
            busy: false,
            serial: 0,
            branch_request: 0,
            diff_request: 0,
            overlay_request: 0,
            mutation_request: 0,
        }
    }
}

pub fn inline(model: &Model) -> bool {
    let panels = &model.panels;
    let sidebar = if panels.sidebar.open {
        panels.sidebar.width
    } else {
        0.0
    };
    !panels.diff.open && panels.window_width - sidebar >= 880.0 && panels.window_height >= 240.0
}

pub fn density(height: f32) -> Density {
    if height >= 540.0 {
        Density::Full
    } else if height >= 420.0 {
        Density::Compact
    } else {
        Density::Essential
    }
}

pub const fn dirty(status: &GitStatus) -> bool {
    status.staged + status.unstaged + status.untracked + status.conflicted > 0
}

pub fn default_branch(model: &Model) -> &str {
    model
        .git
        .default_branch
        .as_deref()
        .or_else(|| {
            model
                .project()
                .and_then(|p| p.settings.base_branch.as_deref())
        })
        .unwrap_or("main")
}
