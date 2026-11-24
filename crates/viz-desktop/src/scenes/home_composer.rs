//! Preview scenes for Phase 1 Track H: centered Home hero states, Project
//! Picker, Home context strip, inline `Mark::Pill` chips, two-row attachment
//! strip, tucked `@`/`/`/`$`/`#` completion drawers, and active-run Steer/Queue
//! CTA + dashed queued deck.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use viz_client::protocol::{
    Access, AccessKind, AccessMode, AccessSource, Attachment, CommandSource, Environment,
    EnvironmentStatus, FileSearchKind, FileSearchMatch, GitHubItem, GitHubItemKind, GitHubPull,
    GitHubPullState, Harness, HarnessStatus, Project, ProjectSettings, QueuedInput, RunId,
    SlashCommand, ThreadStatus, WorktreeTarget,
};

use super::{
    data::{THREAD, options},
    fixtures::{seed_checkouts, seed_workspace},
};
use crate::{
    app::Desktop,
    model::{AttachmentStatus, DraftPill, NewThread},
    update::HOME_PROJECT_PICKER,
};

/// Tiny 8×8 RGB PNG encoded as base64 for deterministic attachment
/// thumbnail previews.
fn sample_png_base64() -> String {
    "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAnklEQVR4nA3JoQ0EIAwAwN/lx0B1hoYhGmZAdYImdQQJFoEpEluC+oWes/f5ggaoCD3BYLACe4If+H2+UUOsGHuKg6OVuGf0E1+QBqpIPdFgskJ7kh96kTXkirmnPDhbyXtmP/mFaJCK0pMMFiuyp/iRF01Dq9h6aoOblbZn89NeLA2r4uppDV5W1p7Lz3pxNdyKt6c7+Fq5e14/9/cHN0FsYWJ76AgAAAAASUVORK5CYII=".to_owned()
}

fn preview_access_modes() -> Vec<AccessMode> {
    vec![
        AccessMode {
            kind: AccessKind::Ask,
            label: "Supervised".into(),
            native: None,
        },
        AccessMode {
            kind: AccessKind::AcceptEdits,
            label: "Auto-accept edits".into(),
            native: None,
        },
        AccessMode {
            kind: AccessKind::Full,
            label: "Full access".into(),
            native: None,
        },
        AccessMode {
            kind: AccessKind::ReadOnly,
            label: "Plan / Read-only".into(),
            native: None,
        },
    ]
}

fn seed_home_harnesses(s: &mut Desktop) {
    s.model.cache_mut().harnesses = vec![Harness {
        id: "claude".into(),
        name: "Claude Code".into(),
        default: true,
        icon: "claude".into(),
        status: HarnessStatus::Ready,
        checked_at: None,
        options: options(),
        access: preview_access_modes(),
        commands: vec![
            SlashCommand {
                name: "review".into(),
                description: "Review unstaged changes in the working tree".into(),
                hint: Some("[path]".into()),
                source: CommandSource::Command,
            },
            SlashCommand {
                name: "rust-best-practices".into(),
                description: "Repo idioms and strict clippy checklist".into(),
                hint: None,
                source: CommandSource::Skill,
            },
        ],
    }];
}

/// Home State A: No project selected, centered hero + composer + `"Full
/// access"`.
pub(super) fn home_no_project(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    seed_home_harnesses(s);
    s.model.selection.selected = None;
    s.model.selection.new_thread = Some(NewThread::new(1, None));
    s.model.composer.draft_focused = false;
    s.sync_widgets_from_model();
}

/// Home State A with the searchable `288px` Project Picker combobox open.
pub(super) fn home_project_menu(s: &mut Desktop) {
    home_no_project(s);
    if let Some(project) = s.model.cache_mut().project_mut("/work/workbench") {
        project.settings.pinned = true;
    }
    s.model.cache_mut().push_project(Project {
        settings: ProjectSettings::default(),
        name: "Release scripts".into(),
        path: "/work/release-scripts".into(),
        created_at: 0,
        git: false,
        branch: None,
    });
    s.model.composer.open_picker = Some(HOME_PROJECT_PICKER.into());
    s.sync_widgets_from_model();
}

/// Home State B: Project selected (`"What should we build in Workbench?"`)
/// with the bottom Context Strip showing Environment, Worktree mode, Branch,
/// and Access Mode.
pub(super) fn home_project(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    seed_home_harnesses(s);
    seed_checkouts(&mut s.model);
    s.model.connection.environments = vec![
        Environment {
            id: "local".into(),
            url: None,
            status: EnvironmentStatus::Local,
        },
        Environment {
            id: "build-box".into(),
            url: Some("https://build-box.tailnet.ts.net".into()),
            status: EnvironmentStatus::Reachable,
        },
    ];
    let project = s.model.cache().project("/work/workbench").cloned();
    let mut draft = NewThread::new(2, project.as_ref());
    draft.worktree = Some(WorktreeTarget::New {
        base: Some("main".into()),
        branch: None,
    });
    s.model.selection.selected = None;
    s.model.selection.new_thread = Some(draft);
    s.model.composer.draft_focused = true;
    s.sync_widgets_from_model();
}

/// Home State C: Zero projects configured on first launch.
pub(super) fn home_empty(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    seed_home_harnesses(s);
    let cache = s.model.cache_mut();
    cache.clear_projects();
    cache.clear_threads();
    cache.open_threads.clear();
    s.model.selection.selected = None;
    s.model.selection.new_thread = None;
    s.sync_widgets_from_model();
}

/// Active thread composer showcasing inline atomic `@file`, `$skill`, and `#pr`
/// `Mark::Pill` chips plus Row A `64×64` media thumbnails and Row B `28px` file
/// attachment rows.
pub(super) fn composer_chips(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let text = "Inspect @markdown.rs with $rust-review for #142 and summarize the diff.";
    let file_start = text.find("@markdown.rs").unwrap_or(0);
    let file_end = file_start + "@markdown.rs".len();
    let skill_start = text.find("$rust-review").unwrap_or(0);
    let skill_end = skill_start + "$rust-review".len();
    let pr_start = text.find("#142").unwrap_or(0);
    let pr_end = pr_start + "#142".len();

    let pr = GitHubItem {
        kind: GitHubItemKind::PullRequest,
        number: 142,
        title: "Preserve final newline in markdown export".into(),
        url: "https://github.com/acme/workbench/pull/142".into(),
        state: "open".into(),
    };
    s.model.composer.draft.text = text.into();
    s.model.composer.draft.pills = vec![
        DraftPill {
            range: file_start..file_end,
            id: DraftPill::file_id("src/export/markdown.rs", false),
            label: "@markdown.rs".into(),
        },
        DraftPill {
            range: skill_start..skill_end,
            id: DraftPill::skill_id("rust-review"),
            label: "$rust-review".into(),
        },
        DraftPill {
            range: pr_start..pr_end,
            id: DraftPill::github_id(&pr),
            label: "#142".into(),
        },
    ];
    let png = sample_png_base64();
    s.model.composer.draft.attachments = vec![
        Attachment {
            name: "timeline-before.png".into(),
            mime: "image/png".into(),
            data: png.clone(),
        },
        Attachment {
            name: "composer-mock.png".into(),
            mime: "image/png".into(),
            data: png,
        },
        Attachment {
            name: "export-trace.log".into(),
            mime: "text/plain".into(),
            data: STANDARD.encode("line 1\nline 2\nexport ok\n".repeat(120)),
        },
        Attachment {
            name: "schema.json".into(),
            mime: "application/json".into(),
            data: STANDARD.encode("{\"version\":2,\"methods\":[]}\n".repeat(45)),
        },
    ]
    .into();
    s.model.composer.draft.attachment_statuses = vec![
        AttachmentStatus::Ready,
        AttachmentStatus::Uploading(Some(68)),
        AttachmentStatus::Ready,
        AttachmentStatus::Ready,
    ];
    s.model.composer.draft_focused = true;
    s.sync_widgets_from_model();
}

/// Tucked `@` mention drawer with ranked files (character highlights + folder)
/// and recent threads.
pub(super) fn composer_at(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.composer.draft.text = "@exp".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_files(
        "exp",
        vec![
            FileSearchMatch {
                path: "src/export/markdown.rs".into(),
                kind: FileSearchKind::File,
                score: 120,
                positions: vec![4, 5, 6],
            },
            FileSearchMatch {
                path: "src/export".into(),
                kind: FileSearchKind::Directory,
                score: 110,
                positions: vec![4, 5, 6],
            },
            FileSearchMatch {
                path: "crates/workbench/src/export/line_ending.rs".into(),
                kind: FileSearchKind::File,
                score: 95,
                positions: vec![21, 22, 23],
            },
        ],
        0,
    );
    s.sync_widgets_from_model();
}

/// Tucked `/` slash-command drawer with built-ins (`/model`, `/clear`,
/// `/compact`) and ACP harness commands.
pub(super) fn composer_slash(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(state) = s.model.cache_mut().open_thread_mut(THREAD).state.as_mut() {
        state.commands = vec![
            SlashCommand {
                name: "review".into(),
                description: "Review unstaged changes in the working tree".into(),
                hint: Some("[path]".into()),
                source: CommandSource::Command,
            },
            SlashCommand {
                name: "plan".into(),
                description: "Draft a step-by-step plan before making edits".into(),
                hint: None,
                source: CommandSource::Command,
            },
            SlashCommand {
                name: "design".into(),
                description: "Generate a UI design review from a brief".into(),
                hint: None,
                source: CommandSource::Skill,
            },
        ];
    }
    s.model.composer.draft.text = "/".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_slash("", 0);
    s.sync_widgets_from_model();
}

/// Tucked `$` skill drawer showing provider/repo skills with
/// `SkillSourceBadge`.
pub(super) fn composer_dollar(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(state) = s.model.cache_mut().open_thread_mut(THREAD).state.as_mut() {
        state.commands = vec![
            SlashCommand {
                name: "rust-best-practices".into(),
                description: "Repo idioms, clippy hygiene, and error modeling".into(),
                hint: None,
                source: CommandSource::Skill,
            },
            SlashCommand {
                name: "ui-audit".into(),
                description: "Project visual layout and contrast review".into(),
                hint: None,
                source: CommandSource::Skill,
            },
            SlashCommand {
                name: "git-release".into(),
                description: "Personal changelog and tag checklist".into(),
                hint: None,
                source: CommandSource::Skill,
            },
        ];
    }
    s.model.composer.draft.text = "$".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_skills("", 0);
    s.sync_widgets_from_model();
}

/// Tucked `#` pull-request and issue mention drawer showing linked PR first
/// followed by repo PRs and issues.
pub(super) fn composer_hash(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(summary) = s.model.cache_mut().summary_mut(THREAD) {
        summary.pull_request = Some(GitHubPull {
            number: 142,
            title: "Preserve final newline in markdown export".into(),
            url: "https://github.com/acme/workbench/pull/142".into(),
            state: GitHubPullState::Open,
            draft: false,
            head: "fix/final-newline".into(),
            head_commit: "abc1234".into(),
            base: "main".into(),
            additions: 24,
            deletions: 6,
            mergeable: "mergeable".into(),
            checks: Some("passing".into()),
            review: Some("approved".into()),
        });
    }
    s.model.composer.draft.text = "#".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_github(
        "",
        vec![
            GitHubItem {
                kind: GitHubItemKind::PullRequest,
                number: 139,
                title: "Add CRLF line-ending preservation for Windows exports".into(),
                url: "https://github.com/acme/workbench/pull/139".into(),
                state: "draft".into(),
            },
            GitHubItem {
                kind: GitHubItemKind::PullRequest,
                number: 131,
                title: "Refactor markdown block joiner".into(),
                url: "https://github.com/acme/workbench/pull/131".into(),
                state: "merged".into(),
            },
            GitHubItem {
                kind: GitHubItemKind::Issue,
                number: 128,
                title: "Export strips trailing newline when last block is fenced code".into(),
                url: "https://github.com/acme/workbench/issues/128".into(),
                state: "open".into(),
            },
        ],
        0,
    );
    s.sync_widgets_from_model();
}

/// Active running thread with Steer/Queue/Stop CTA, non-default Access Mode
/// chip, and right-aligned dashed `QueuedRunsControl` stack.
pub(super) fn composer_queued(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(state) = s.model.cache_mut().open_thread_mut(THREAD).state.as_mut() {
        state.status = ThreadStatus::Working;
        state.access = Access {
            current: AccessMode {
                kind: AccessKind::AcceptEdits,
                label: "Auto-accept edits".into(),
                native: None,
            },
            source: AccessSource::Thread,
            modes: preview_access_modes(),
        };
        state.queue = vec![
            QueuedInput {
                run: RunId(501),
                text: "Also check whether fenced code blocks preserve CRLF endings.".into(),
                attachment_count: 0,
            },
            QueuedInput {
                run: RunId(502),
                text: "Update the CLI flag help text once the exporter passes.".into(),
                attachment_count: 1,
            },
        ];
    }
    s.model.composer.draft.text = "Focus on src/export/markdown.rs first.".into();
    s.model.composer.draft_focused = true;
    s.sync_widgets_from_model();
}

/// Active thread composer showcasing live WYSIWYG-style Markdown decorations:
/// heading, bold, italic, strikethrough, inline code, link, blockquote, lists,
/// task items, and language-highlighted fenced code block.
pub(super) fn composer_markdown(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let text = "\
## Ship live Markdown
Use **bold**, *italic*, ~~legacy toolbar~~, `inline_code()`, and [docs](https://example.com).
> Quoted note with **emphasis** and `tokens`
- [x] Remove formatting buttons
- [ ] Highlight fenced blocks
```rust
fn greet(name: &str) -> String {
    format!(\"hello {name}\")
}
```";
    s.model.composer.draft.text = text.into();
    s.model.composer.draft_focused = true;
    s.sync_widgets_from_model();
}
