//! Dedicated preview scenes exercising every state of the unified triage
//! sidebar across themes: populated shelves, project filter combobox, hover
//! preview card, 4-group context menu with submenu, inline rename + undo toast,
//! and collapsed sidebar with unread badge.

use rune::TextEdit;
use viz_client::protocol::{
    Attention, GitHubPull, GitHubPullState, Harness, HarnessStatus, Lifecycle, Project,
    ProjectIcon, ProjectSettings, SettledBy, Shelf, Standing, Task, TaskStatus, ThreadId,
    ThreadStatus, ThreadSummary, ThreadWorktree, WorktreeTarget,
};

use super::{
    data::now_ms,
    fixtures::{seed_workspace, select_preview},
};
use crate::{
    app::Desktop,
    model::{
        ContextMenu, ContextSubmenu, InlineRename, Model, NewThread, ProjectFilter, SHELF_SETTLED,
        SHELF_SNOOZED, UndoTriage,
    },
};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;

/// Populated unified triage sidebar showing Drafts, Pinned, Working, Active,
/// Snoozed, and an expanded bottom-anchored Settled drawer with pagination.
pub(super) fn triage(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    s.model
        .cache_mut()
        .retain_threads(|t| matches!(t.id, ThreadId(40 | 38 | 34 | 29)) || t.id.0 >= 100);
    let mut draft = NewThread::new(1, s.model.cache().project("/work/workbench"));
    draft.worktree = Some(WorktreeTarget::New {
        base: Some("main".into()),
        branch: Some("viz/sidebar-polish".into()),
    });
    s.model.selection.new_thread = Some(draft);
    s.model.composer.draft.text = "Polish sidebar triage hover transitions".into();
    s.model.sidebar.collapsed_shelves.remove(SHELF_SNOOZED);
    s.model.sidebar.collapsed_shelves.remove(SHELF_SETTLED);
    s.model.sidebar.settled_limit = 2;
    s.sync_widgets_from_model();
}

/// Active project filter chip plus open searchable Project Filter Combobox.
pub(super) fn filter(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    s.model.sidebar.project_filter = ProjectFilter::Project("/work/workbench".into());
    s.model.sidebar.filter_open = true;
    s.model.sidebar.filter_active = 1;
    s.sync_widgets_from_model();
}

/// Delayed right-anchored `ThreadHoverCard` open beside a thread card with
/// zero-shift hover quick actions revealed.
pub(super) fn hover(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    s.model.sidebar.hover_card = Some(ThreadId(32));
    s.sync_widgets_from_model();
}

/// Full 4-group thread context menu with the Snooze submenu expanded.
pub(super) fn context(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    s.model.sidebar.menu = Some(ContextMenu::preview(
        format!("thread-{}", ThreadId(32)),
        132.0,
        248.0,
        Some(ContextSubmenu::Snooze),
    ));
    s.sync_widgets_from_model();
}

/// Inline title rename input active on Row 2 alongside a 5-second triage undo
/// toast at the bottom-right corner.
pub(super) fn undo(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    let draft_title = "Fix CRLF export whitespace";
    s.model.sidebar.inline_rename = Some(InlineRename {
        thread: ThreadId(32),
        value: draft_title.into(),
    });
    s.widgets.inline_rename = TextEdit::new(draft_title);
    s.widgets.inline_rename.focused = true;
    s.sync_widgets_from_model();
    s.preview_undo_toast(
        "Thread settled",
        "Audit error messages",
        UndoTriage {
            thread: ThreadId(34),
            prior: Lifecycle::default(),
        },
    );
}

/// Collapsed sidebar revealing the toolbar `PanelLeft` toggle with an unread
/// indicator dot.
pub(super) fn collapsed(s: &mut Desktop) {
    seed_triage_workspace(&mut s.model);
    s.model.panels.sidebar.open = false;
    s.sync_widgets_from_model();
    s.widgets.sidebar_pane.set_open(false);
}

#[expect(
    clippy::too_many_lines,
    reason = "single deterministic fixture seeding all triage shelves, attention states, PR badges, and icon variants"
)]
fn seed_triage_workspace(m: &mut Model) {
    seed_workspace(m);
    let now = now_ms();
    let projects: Vec<Project> = [
        (
            "Workbench",
            "/work/workbench",
            30,
            Some("main"),
            true,
            ProjectIcon::Lucide {
                name: "folder-code".into(),
                color: "purple".into(),
            },
        ),
        (
            "Rune UI",
            "/work/rune-ui",
            20,
            Some("main"),
            true,
            ProjectIcon::Lucide {
                name: "cpu".into(),
                color: "green".into(),
            },
        ),
        (
            "Field notes",
            "/work/field-notes",
            10,
            Some("drafts"),
            false,
            ProjectIcon::Emoji {
                emoji: "⚡".into()
            },
        ),
        (
            "Release scripts",
            "/work/release-scripts",
            5,
            None,
            false,
            ProjectIcon::Automatic,
        ),
    ]
    .into_iter()
    .map(|(name, path, days, branch, pinned, icon)| Project {
        name: name.into(),
        path: path.into(),
        created_at: now - days * 24 * HOUR_MS,
        git: branch.is_some(),
        branch: branch.map(str::to_owned),
        settings: ProjectSettings {
            pinned,
            icon,
            ..ProjectSettings::default()
        },
    })
    .collect();

    let mut threads = vec![
        ThreadSummary {
            pull_request: Some(sample_pr(214, GitHubPullState::Open, false)),
            worktree: Some(sample_wt("viz-sidebar", "feat/triage-sidebar")),
            model: Some("Claude Opus 4.7".into()),
            thinking: Some("high".into()),
            lifecycle: Lifecycle {
                pin: Some(0),
                ..Lifecycle::default()
            },
            shelf: Shelf::Pinned,
            ..base_thread(
                40,
                "Ship unified sidebar layout",
                Some("/work/workbench"),
                "claude",
                now - 4 * MINUTE_MS,
            )
        },
        ThreadSummary {
            model: Some("GPT-5.4".into()),
            thinking: Some("medium".into()),
            lifecycle: Lifecycle {
                pin: Some(1),
                unread: true,
                ..Lifecycle::default()
            },
            shelf: Shelf::Pinned,
            attention: Some(Attention::Unread),
            ..base_thread(
                39,
                "Audit hover delay cancellations",
                Some("/work/rune-ui"),
                "codex",
                now - 18 * MINUTE_MS,
            )
        },
        ThreadSummary {
            pull_request: Some(sample_pr(218, GitHubPullState::Open, true)),
            worktree: Some(sample_wt("viz-migration", "viz/migration-0014")),
            model: Some("Claude Sonnet 4.6".into()),
            thinking: Some("high".into()),
            status: ThreadStatus::Working,
            shelf: Shelf::Working,
            attention: Some(Attention::Approval),
            ..base_thread(
                38,
                "Approve migration 0014 schema",
                Some("/work/workbench"),
                "claude",
                now - 2 * MINUTE_MS,
            )
        },
        ThreadSummary {
            model: Some("Gemini 3 Pro".into()),
            status: ThreadStatus::Working,
            shelf: Shelf::Working,
            attention: Some(Attention::Question),
            ..base_thread(
                37,
                "Choose base16 token for snooze badge",
                Some("/work/rune-ui"),
                "agy",
                now - 6 * MINUTE_MS,
            )
        },
        ThreadSummary {
            model: Some("GPT-5.4".into()),
            thinking: Some("high".into()),
            status: ThreadStatus::Working,
            shelf: Shelf::Working,
            waiting_on: 2,
            ..base_thread(
                36,
                "Run headless scene verification",
                Some("/work/workbench"),
                "codex",
                now - 3 * MINUTE_MS,
            )
        },
        ThreadSummary {
            pull_request: Some(sample_pr(209, GitHubPullState::Open, false)),
            worktree: Some(sample_wt("viz-trim", "viz/trim-export-whitespace")),
            model: Some("Claude Opus 4.7".into()),
            thinking: Some("high".into()),
            ..base_thread(
                32,
                "Trim export whitespace",
                Some("/work/workbench"),
                "claude",
                now - 14 * MINUTE_MS,
            )
        },
        ThreadSummary {
            parent: Some(ThreadId(32)),
            model: Some("GPT-5.4".into()),
            thinking: Some("high".into()),
            status: ThreadStatus::Working,
            shelf: Shelf::Working,
            ..base_thread(
                41,
                "Compare markdown exports",
                Some("/work/workbench"),
                "codex",
                now - 2 * MINUTE_MS,
            )
        },
        ThreadSummary {
            parent: Some(ThreadId(32)),
            model: Some("Claude Sonnet 4.6".into()),
            thinking: Some("medium".into()),
            ..base_thread(
                42,
                "Review CRLF edge cases",
                Some("/work/workbench"),
                "claude",
                now - 8 * MINUTE_MS,
            )
        },
        ThreadSummary {
            pull_request: Some(sample_pr(198, GitHubPullState::Merged, false)),
            lifecycle: Lifecycle {
                unread: true,
                ..Lifecycle::default()
            },
            attention: Some(Attention::Unread),
            ..base_thread(
                34,
                "Audit error messages",
                Some("/work/field-notes"),
                "omp",
                now - 42 * MINUTE_MS,
            )
        },
        ThreadSummary {
            pull_request: Some(sample_pr(191, GitHubPullState::Closed, false)),
            status: ThreadStatus::Failed,
            lifecycle: Lifecycle {
                unread: true,
                ..Lifecycle::default()
            },
            attention: Some(Attention::Failure),
            ..base_thread(
                33,
                "Organize reference links",
                Some("/work/release-scripts"),
                "pi",
                now - 2 * HOUR_MS,
            )
        },
        base_thread(31, "Draft release notes", None, "claude", now - 5 * HOUR_MS),
        ThreadSummary {
            lifecycle: Lifecycle {
                snoozed_until: Some(now + 45 * MINUTE_MS),
                ..Lifecycle::default()
            },
            shelf: Shelf::Snoozed,
            ..base_thread(
                29,
                "Revisit Tailscale cert renewal",
                Some("/work/release-scripts"),
                "codex",
                now - 3 * HOUR_MS,
            )
        },
        ThreadSummary {
            lifecycle: Lifecycle {
                snoozed_until: Some(now + 26 * HOUR_MS),
                ..Lifecycle::default()
            },
            shelf: Shelf::Snoozed,
            ..base_thread(
                28,
                "Check macOS traffic light offset",
                Some("/work/rune-ui"),
                "claude",
                now - 9 * HOUR_MS,
            )
        },
    ];

    for idx in 0..18_u64 {
        let settled_at = now - i64::try_from(idx + 1).unwrap_or(1) * 3 * HOUR_MS;
        let (project, harness) = if idx % 2 == 0 {
            (Some("/work/workbench"), "claude")
        } else {
            (Some("/work/rune-ui"), "codex")
        };
        let standing = Standing::Settled {
            at: settled_at,
            by: SettledBy::User,
        };
        threads.push(ThreadSummary {
            lifecycle: Lifecycle {
                standing,
                ..Lifecycle::default()
            },
            shelf: Shelf::Settled,
            ..base_thread(
                100 + idx,
                &format!("Completed triage task #{}", idx + 1),
                project,
                harness,
                settled_at,
            )
        });
    }

    let cache = m.cache_mut();
    cache.set_projects(projects);
    cache.set_threads(threads);
    cache.harnesses = vec![Harness {
        id: "claude".into(),
        name: "Claude Code".into(),
        icon: "claude".into(),
        status: HarnessStatus::Ready,
        checked_at: None,
        default: true,
        options: Vec::new(),
        commands: Vec::new(),
        access: Vec::new(),
    }];
    cache.tasks = vec![
        sample_task(
            "41",
            32,
            41,
            TaskStatus::Running,
            "Compare markdown exports",
        ),
        sample_task(
            "42",
            32,
            42,
            TaskStatus::Completed,
            "Review CRLF edge cases",
        ),
    ];

    select_preview(m, ThreadId(32));
    let open = m.cache_mut().open_thread_mut(ThreadId(32));
    open.state = Some(super::data::thread_state());
    open.transcript = super::data::transcript(&[]);
    open.loading = false;
}

fn base_thread(
    id: u64,
    title: &str,
    project: Option<&str>,
    harness: &str,
    updated_at: i64,
) -> ThreadSummary {
    let harness_name = match harness {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "omp" => "Oh My Pi",
        "pi" => "Pi",
        _ => "Google Antigravity",
    };
    ThreadSummary {
        id: ThreadId(id),
        title: title.into(),
        parent: None,
        parent_environment: None,
        project: project.map(str::to_owned),
        pull_request: None,
        worktree: None,
        harness: harness.into(),
        harness_name: harness_name.into(),
        model: None,
        thinking: None,
        status: ThreadStatus::Idle,
        updated_at,
        lifecycle: Lifecycle::default(),
        shelf: Shelf::Active,
        attention: None,
        waiting_on: 0,
        forked_from: None,
        last_input_at: Some(updated_at),
    }
}

fn sample_wt(slug: &str, branch: &str) -> ThreadWorktree {
    ThreadWorktree {
        path: format!("/state/worktrees/workbench/{slug}"),
        branch: Some(branch.into()),
    }
}

fn sample_task(id: &str, parent: u64, thread: u64, status: TaskStatus, title: &str) -> Task {
    Task {
        id: id.into(),
        parent: ThreadId(parent),
        environment: "local".into(),
        thread: Some(ThreadId(thread)),
        run: None,
        status,
        reachable: true,
        title: title.into(),
        result: (status == TaskStatus::Completed).then(|| "Completed cleanly.".into()),
        error: None,
    }
}

fn sample_pr(number: u64, state: GitHubPullState, draft: bool) -> GitHubPull {
    let title = match number {
        214 => "Unified triage sidebar layout",
        218 => "Add thread lifecycle migration 0014",
        209 => "Trim trailing export whitespace",
        198 => "Audit CLI error message formatting",
        _ => "Update reference documentation links",
    };
    GitHubPull {
        number,
        title: title.into(),
        url: format!("https://github.com/viz/workbench/pull/{number}"),
        state,
        draft,
        head: "feat/branch".into(),
        head_commit: "deadbeef".into(),
        base: "main".into(),
        additions: 120,
        deletions: 34,
        mergeable: "MERGEABLE".into(),
        checks: Some("SUCCESS".into()),
        review: None,
    }
}
