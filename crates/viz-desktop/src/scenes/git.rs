//! Deterministic source-control surfaces for headless visual review.

use rune_kit::diff::DiffMode;
use viz_client::protocol::{
    BranchList, ContextUsage, GitBranch, GitDiff, GitDiffFile, GitDiffLine, GitFileKind,
    GitHubCheck, GitHubPull, GitHubPullState, GitHunk, GitLineStats, GitStatus,
};

use super::{
    data::THREAD,
    fixtures::{seed_checkouts, seed_workspace},
};
use crate::{
    app::Desktop,
    model::git::{Dialog, Popup},
};

pub fn full(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    seed_checkouts(&mut s.model);
    let (path, head) = s
        .model
        .summary(THREAD)
        .and_then(|thread| thread.worktree.as_ref())
        .map_or_else(
            || ("/work/workbench".into(), "feature/export".into()),
            |w| {
                (
                    w.path.clone(),
                    w.branch.clone().unwrap_or_else(|| "feature/export".into()),
                )
            },
        );
    let cache = s.model.cache_mut();
    if let Some(thread) = cache.summary_mut(THREAD) {
        thread.pull_request = Some(GitHubPull {
            number: 42,
            title: "Normalize report exports".into(),
            url: "https://github.com/example/workbench/pull/42".into(),
            state: GitHubPullState::Open,
            draft: false,
            head: head.clone(),
            head_commit: "78a4bbd".into(),
            base: "main".into(),
            additions: 7,
            deletions: 2,
            mergeable: "MERGEABLE".into(),
            checks: None,
            review: None,
        });
    }
    if let Some(state) = &mut cache.open_thread_mut(THREAD).state {
        state.usage = Some(ContextUsage {
            used: 28_400,
            size: 128_000,
        });
    }
    s.model.git.watch = Some((THREAD, path.clone()));
    s.model.git.status = Some(GitStatus {
        checkout: path,
        branch: Some(head),
        commit: Some("78a4bbd".into()),
        upstream: Some("origin/feature/export".into()),
        ahead: 1,
        behind: 0,
        staged: 1,
        unstaged: 2,
        untracked: 0,
        conflicted: 0,
        stats: GitLineStats {
            added: 7,
            removed: 2,
        },
        operation: None,
    });
    s.model.panels.details_open = true;
    s.sync_widgets_from_model();
}

pub fn compact(s: &mut Desktop) {
    full(s);
    s.model.panels.window_height = 570.0;
}

pub fn essential(s: &mut Desktop) {
    full(s);
    s.model.panels.window_height = 490.0;
}

pub fn branch(s: &mut Desktop) {
    full(s);
    s.model.git.popup = Some(Popup::Branch);
    s.model.git.query = "export".into();
    s.model.git.branches = Some(BranchList {
        branches: ["feature/export", "origin/feature/export", "export-reader"]
            .into_iter()
            .map(|name| GitBranch {
                name: name.into(),
                remote: name.starts_with("origin/"),
                upstream: None,
                checked_out_elsewhere: false,
            })
            .collect(),
        default: Some("main".into()),
        next: Some(50),
    });
    s.sync_widgets_from_model();
}

pub fn actions(s: &mut Desktop) {
    full(s);
    s.model.git.popup = Some(Popup::Actions);
}

pub fn commit(s: &mut Desktop) {
    full(s);
    s.model.git.dialog = Some(Dialog::Commit);
    s.model.git.preview = Some(example_diff());
    s.widgets.git_message = rune::TextEdit::new("Normalize report exports");
}

pub fn checks(s: &mut Desktop) {
    full(s);
    s.model.git.popup = Some(Popup::Checks);
    s.model.git.checks = Some(vec![
        GitHubCheck {
            id: 1,
            name: "Build / Linux".into(),
            status: "completed".into(),
            conclusion: Some("success".into()),
            url: None,
        },
        GitHubCheck {
            id: 2,
            name: "Format".into(),
            status: "in_progress".into(),
            conclusion: None,
            url: None,
        },
    ]);
}

pub fn unified(s: &mut Desktop) {
    full(s);
    s.model.panels.details_open = false;
    s.model.panels.diff.open = true;
    s.model.panels.diff.width = 550.0;
    s.model.git.diff = Some(example_diff().into());
    s.widgets.git_tree_open = true;
    s.sync_widgets_from_model();
}

pub fn split(s: &mut Desktop) {
    unified(s);
    s.widgets.git_diff.set_mode(DiffMode::Split);
}

fn example_diff() -> GitDiff {
    GitDiff {
        revision: "78a4bbd".into(),
        stats: GitLineStats {
            added: 4,
            removed: 2,
        },
        truncated: false,
        files: ["src/export.rs", "src/report.rs"]
            .into_iter()
            .map(|path| GitDiffFile {
                old_path: Some(path.into()),
                new_path: Some(path.into()),
                kind: GitFileKind::Modified,
                binary: false,
                truncated: false,
                stats: Some(GitLineStats {
                    added: 2,
                    removed: 1,
                }),
                hunks: vec![GitHunk {
                    old_start: 1,
                    old_lines: 3,
                    new_start: 1,
                    new_lines: 4,
                    header: String::new(),
                    lines: [
                        (' ', Some(1), Some(1), "fn write_report(report: &Report) {"),
                        ('-', Some(2), None, "    render(report);"),
                        ('+', None, Some(2), "    let report = report.normalize();"),
                        ('+', None, Some(3), "    render(&report);"),
                        (' ', Some(3), Some(4), "}"),
                    ]
                    .into_iter()
                    .map(|(origin, old_line, new_line, content)| GitDiffLine {
                        origin,
                        old_line,
                        new_line,
                        content: content.into(),
                    })
                    .collect(),
                }],
            })
            .collect(),
    }
}
