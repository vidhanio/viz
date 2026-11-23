//! Preview scenes for menus, palette, dialogs, drafts, completions,
//! environments, and empty/offline states.

use viz_client::protocol::{
    Attachment, EntryId, Environment, EnvironmentStatus, Harness, Project, SearchHit, SlashCommand,
    ThreadId, WorktreeTarget,
};

use super::{
    data::{THREAD, option, options, transcript},
    fixtures::{seed_checkouts, seed_workspace, select_preview},
};
use crate::{
    app::Desktop,
    checkout,
    model::{
        ContextMenu, ContextSubmenu, NewThread, PaletteSearch, ProjectFilter, SidebarEdit, Status,
        Transcript,
    },
    timeline,
};

pub(super) fn new_agent(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let draft = NewThread::new(0, s.model.project());
    s.model.selection.selected = None;
    s.model.selection.new_thread = Some(draft);
    s.model.cache_mut().harnesses = vec![
        Harness {
            icon: "omp".into(),
            status: viz_client::protocol::HarnessStatus::Ready,
            checked_at: None,
            access: Vec::new(),
            commands: Vec::new(),
            id: "omp".into(),
            name: "Oh My Pi".into(),
            default: false,
            options: options(),
        },
        Harness {
            icon: "codex".into(),
            status: viz_client::protocol::HarnessStatus::Ready,
            checked_at: None,
            access: Vec::new(),
            commands: Vec::new(),
            id: "codex".into(),
            name: "Codex".into(),
            default: false,
            options: Vec::new(),
        },
        Harness {
            icon: "gemini".into(),
            status: viz_client::protocol::HarnessStatus::Ready,
            checked_at: None,
            access: Vec::new(),
            commands: Vec::new(),
            id: "agy".into(),
            name: "Google Antigravity".into(),
            default: true,
            options: vec![option(
                "model",
                "Model",
                "model",
                "gemini-4-neon",
                &[("gemini-4-neon", "Gemini 4 Neon")],
            )],
        },
    ];
    s.sync_widgets_from_model();
}

pub(super) fn context_menu(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.sidebar.menu = Some(ContextMenu::preview(
        format!("thread-{}", ThreadId(9)),
        120.0,
        280.0,
        Some(ContextSubmenu::MoveToProject),
    ));
    s.sync_widgets_from_model();
}

pub(super) fn palette(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let hits = [
        (
            11,
            1,
            "Make the markdown export preserve a final \u{1}newline\u{2}.",
        ),
        (
            11,
            2,
            "The export drops the final \u{1}newline\u{2} when it joins blocks.",
        ),
        (
            9,
            14,
            "Error messages end without a trailing \u{1}newline\u{2} on Windows",
        ),
    ]
    .map(|(thread, entry, snippet)| SearchHit {
        thread: ThreadId(thread),
        entry: EntryId(entry),
        snippet: snippet.into(),
    })
    .into();
    preview_palette(s, "newline", hits);
}

pub(super) fn palette_open(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    preview_palette(s, "", Vec::new());
}

fn preview_palette(s: &mut Desktop, query: &str, hits: Vec<SearchHit>) {
    let palette = &mut s.model.palette;
    palette.open = true;
    palette.query = query.into();
    palette.filter = query.trim().to_lowercase();
    palette.search = PaletteSearch::Complete(Ok(hits));
    s.sync_widgets_from_model();
}

pub(super) fn attachments(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.cache_mut().open_thread_mut(THREAD).transcript =
        transcript(&["export-before.png", "notes.md"]);
    s.model.composer.draft.attachments = ["Screenshot 2026-11-18.png", "export.log"]
        .map(|name| Attachment {
            name: name.into(),
            mime: "image/png".into(),
            data: String::new(),
        })
        .into();
    s.sync_widgets_from_model();
}

pub(super) fn empty(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.cache_mut().open_thread_mut(THREAD).transcript = Transcript::default();
    s.sync_widgets_from_model();
}

pub(super) fn loading(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    select_preview(&mut s.model, ThreadId(10));
    s.sync_widgets_from_model();
}

pub(super) fn onboarding(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let cache = s.model.cache_mut();
    cache.clear_projects();
    cache.clear_threads();
    cache.open_threads.clear();
    s.model.selection.selected = None;
    s.sync_widgets_from_model();
}

pub(super) fn projects(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.cache_mut().push_project(Project {
        settings: viz_client::protocol::ProjectSettings::default(),
        name: "Release scripts".into(),
        path: "/work/release-scripts".into(),
        created_at: 0,
        git: false,
        branch: None,
    });
    s.model.sidebar.project_filter = ProjectFilter::Project("/work/workbench".into());
    s.sync_widgets_from_model();
}

pub(super) fn draft_project(s: &mut Desktop) {
    new_agent(s);
    s.model.composer.open_picker = Some(timeline::DRAFT_PROJECT.into());
    s.sync_widgets_from_model();
}

pub(super) fn draft_worktree(s: &mut Desktop) {
    new_agent(s);
    seed_checkouts(&mut s.model);
    if let Some(draft) = &mut s.model.selection.new_thread {
        draft.worktree = Some(WorktreeTarget::New {
            base: None,
            branch: None,
        });
    }
    s.model.composer.open_picker = Some(checkout::DRAFT_CHECKOUT.into());
    s.sync_widgets_from_model();
}

pub(super) fn move_thread(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    seed_checkouts(&mut s.model);
    select_preview(&mut s.model, ThreadId(10));
    let open = s.model.cache_mut().open_thread_mut(ThreadId(10));
    open.transcript = transcript(&[]);
    open.loading = false;
    s.model.composer.open_picker = Some(checkout::THREAD_CHECKOUT.into());
    s.sync_widgets_from_model();
}

pub(super) fn environments(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.connection.environments = vec![
        Environment {
            id: "local".into(),
            url: None,
            status: EnvironmentStatus::Local,
        },
        Environment {
            id: "workstation".into(),
            url: Some("https://workstation.tailnet.ts.net".into()),
            status: EnvironmentStatus::Reachable,
        },
        Environment {
            id: "build-box".into(),
            url: Some("https://build-box.tailnet.ts.net".into()),
            status: EnvironmentStatus::Unreachable,
        },
    ];
    s.model.sidebar.env_menu = true;
    s.sync_widgets_from_model();
}

pub(super) fn offline(s: &mut Desktop) {
    s.model.connection.status = Status::Error(
        "connection refused (os error 111): could not reach viz-server at ws://127.0.0.1:7420/ws"
            .into(),
    );
    s.model.connection.endpoint = Some("http://127.0.0.1:7420".into());
    s.sync_widgets_from_model();
}

pub(super) fn delete(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.sidebar.edit = Some(SidebarEdit::DeleteThread(THREAD));
    s.sync_widgets_from_model();
}

pub(super) fn overflow(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(thread) = s.model.cache_mut().summary_mut(THREAD) {
        thread.title = "Which Rust edition does /home/vidhanio/Projects/viz/Cargo.toml use, and \
                      does every crate in the workspace inherit it?"
            .into();
    }
    s.model.sidebar.edit = Some(SidebarEdit::DeleteThread(THREAD));
    s.sync_widgets_from_model();
    s.preview_toast(
        "Couldn’t open /home/vidhanio/Projects/viz/crates/viz-desktop/src/sidebar/actions.rs \
         for writing",
        "error: failed to write \
         /home/vidhanio/Projects/viz/target/preview/export-markdown-notes-4f1c2a/crates/\
         viz-desktop/src/ui/mod.rs: Permission denied (os error 13)\nhint: check the \
         directory’s owner\nhint: or remove it and start the thread again\nnote: the server \
         kept the previous version on disk",
    );
    s.preview_toast("Offline", "The server disconnected.");
}

pub(super) fn slash(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    if let Some(state) = s.model.cache_mut().open_thread_mut(THREAD).state.as_mut() {
        state.commands = vec![
            SlashCommand {
                name: "compact".into(),
                description: "Summarize the conversation to free context window".into(),
                hint: Some("[focus]".into()),
                source: viz_client::protocol::CommandSource::Command,
            },
            SlashCommand {
                name: "review".into(),
                description: "Review unstaged changes in the working tree".into(),
                hint: Some("[path]".into()),
                source: viz_client::protocol::CommandSource::Command,
            },
            SlashCommand {
                name: "plan".into(),
                description: "Draft a step-by-step plan before making edits".into(),
                hint: None,
                source: viz_client::protocol::CommandSource::Command,
            },
        ];
    }
    s.model.composer.draft.text = "/".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_slash("", 0);
    s.sync_widgets_from_model();
}

pub(super) fn mentions(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    s.model.composer.draft.text = "@export".into();
    s.model.composer.draft_focused = true;
    s.model.composer.completion.preview_files(
        "export",
        vec![
            "src/export/markdown.rs".into(),
            "src/export/line_ending.rs".into(),
            "crates/workbench/src/export/mod.rs".into(),
        ],
        1,
    );
    s.sync_widgets_from_model();
}
