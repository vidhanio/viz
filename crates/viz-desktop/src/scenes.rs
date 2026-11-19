//! Deterministic `AppSpec` scenes for inspecting desktop layout without a live
//! socket.

use serde_json::json;
use viz_client::protocol::{
    Attachment, AttachmentCapabilities, Branches, ConfigOption, ConversationId, ConversationKind,
    ConversationState, ConversationStatus, ConversationSummary, DefaultWorkspace, EntryId, GitFile,
    GitFileStatus, GitState, Harness, ManagedWorktree, OptionChoice, OptionKind, OptionValue,
    Project, QueuedInput, SearchHit, SlashCommand, SubmissionId,
};

use crate::{
    app::{Desktop, NewChat, Status},
    changes, palette, sidebar,
    thread::Thread,
};

type Scene = (&'static str, fn(&mut Desktop));
pub const SCENES: &[Scene] = &[
    ("workspace", workspace_review),
    ("workspace-open", workspace_open),
    ("workspace-locked", workspace_locked),
    ("selectors", selectors),
    ("palette", palette),
    ("diff", diff),
    ("commit", commit),
    ("attachments", attachments),
    ("queue", queue),
    ("empty", empty),
    ("onboarding", onboarding),
    ("orchestrator", orchestrator),
    ("subagents", subagents),
    ("agents", agents),
    ("subagent", subagent),
    ("delete", delete),
    ("projects", projects),
];

const CHAT: ConversationId = ConversationId(11);

fn workspace(s: &mut Desktop) {
    s.status = Status::Connected {
        version: "preview".into(),
    };
    s.root = Some(ConversationId(1));
    s.projects = [
        ("workbench", "Workbench", "/work/workbench", "main"),
        ("field-notes", "Field notes", "/work/field-notes", "docs"),
    ]
    .map(|(id, name, path, branch)| Project {
        id: id.into(),
        name: name.into(),
        path: path.into(),
        branch: Some(branch.into()),
        default_workspace: if id == "workbench" {
            DefaultWorkspace::Worktree
        } else {
            DefaultWorkspace::Local
        },
        created_at: 0,
    })
    .into();
    s.conversations = list();
    s.show(CHAT);
    s.draft.focused = true;
    s.conversation = Some(conversation());
    s.thread = transcript(&[]);
    s.toggle("e4");
}

fn list() -> Vec<ConversationSummary> {
    use ConversationKind::{Chat, Orchestrator, Subagent};
    use ConversationStatus::{Failed, Idle, Working};

    [
        (
            21,
            "Review line endings",
            Subagent,
            Some(11),
            Some("workbench"),
            Idle,
        ),
        (
            20,
            "Compare exports",
            Subagent,
            Some(11),
            Some("workbench"),
            Working,
        ),
        (
            11,
            "Export markdown notes",
            Chat,
            None,
            Some("workbench"),
            Idle,
        ),
        (
            10,
            "Trim export whitespace",
            Chat,
            None,
            Some("workbench"),
            Idle,
        ),
        (
            9,
            "Audit error messages",
            Chat,
            None,
            Some("workbench"),
            Idle,
        ),
        (
            7,
            "Organize reference links",
            Chat,
            None,
            Some("field-notes"),
            Failed,
        ),
        (4, "Draft release notes", Chat, None, None, Idle),
        (1, "Orchestrator", Orchestrator, None, None, Idle),
    ]
    .map(
        |(id, title, kind, parent, project, status)| ConversationSummary {
            id: ConversationId(id),
            title: title.into(),
            kind,
            parent: parent.map(ConversationId),
            project: project.map(str::to_owned),
            harness: (kind != Orchestrator).then(|| "omp".into()),
            status,
            archived: false,
            // Minutes ago, newest first by id, so "last activity" reads
            // like a live session.
            updated_at: now_ms() - i64::try_from(30 - id).unwrap_or_default() * 60_000,
            unread: id == 9,
            pinned: id == 4,
            worktree: (id == 10).then(|| "/work/workbench/.viz/worktrees/trim-export".into()),
            branch: (id == 10).then(|| "viz/trim-export".into()),
        },
    )
    .into()
}

fn conversation() -> ConversationState {
    ConversationState {
        status: ConversationStatus::Idle,
        harness: Some("omp".into()),
        project: Some("workbench".into()),
        cwd: Some("/work/workbench".into()),
        read_only: false,
        options: vec![
            option(
                "model",
                "Model",
                "model",
                "opus",
                &[
                    ("opus", "Claude Opus"),
                    ("sonnet", "Claude Sonnet"),
                    ("gpt", "GPT-6"),
                ],
            ),
            option(
                "effort",
                "Effort",
                "thought_level",
                "high",
                &[("low", "Low"), ("high", "High")],
            ),
            option(
                "mode",
                "Mode",
                "mode",
                "build",
                &[("build", "Build"), ("plan", "Plan")],
            ),
            ConfigOption {
                id: "context".into(),
                name: "Include workspace context".into(),
                category: None,
                description: None,
                kind: OptionKind::Boolean,
                value: OptionValue::Boolean(true),
                choices: Vec::new(),
            },
        ],
        commands: vec![SlashCommand {
            name: "review".into(),
            description: "Review the current changes".into(),
            hint: Some("[path]".into()),
        }],
        queue: Vec::new(),
        git: Some(GitState {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead: 2,
            behind: 0,
            worktree: None,
            files: [
                ("backend/src/routes.ts", GitFileStatus::Modified),
                ("docs/glossary.md", GitFileStatus::Untracked),
            ]
            .map(|(path, status)| GitFile {
                path: path.into(),
                status,
                staged: false,
            })
            .into(),
        }),
        attachments: AttachmentCapabilities { images: true },
    }
}

fn transcript(attachments: &[&str]) -> Thread {
    let attachments: Vec<_> = attachments
        .iter()
        .map(|name| json!({"name": name, "mime": "image/png", "path": format!("/state/attachments/{name}")}))
        .collect();
    Thread::from_view(&json!({
        "entries": [
            {"id":1,"kind":"viz.agent.prompt","data":{"text":"Make the markdown export preserve a final newline.","attachments":attachments}},
            {"id":2,"kind":"viz.agent.text","data":{"text":"The export drops the final newline when it joins blocks. I’ll fix that at the boundary."}},
            {"id":3,"kind":"viz.agent.plan","data":{"entries":[
                {"content":"Read the export pipeline","status":"completed"},
                {"content":"Preserve the final newline","status":"completed"},
                {"content":"Check export output","status":"in_progress"}
            ]}},
            {"id":4,"kind":"viz.agent.tool","data":{
                "id":"write-1","title":"Update markdown export","kind":"edit","status":"completed",
                "output":"Updated 1 file.",
                "diffs":[{"path":"src/export.rs","oldText":"let body = blocks.join(\"\\n\\n\");\nOk(body)\n","newText":"let body = blocks.join(\"\\n\\n\");\nOk(format!(\"{body}\\n\"))\n"}]
            }},
            {"id":5,"kind":"viz.agent.turn","data":{"stopReason":"end_turn","error":null}}
        ],
        "before": 40,
    }))
}

fn selectors(s: &mut Desktop) {
    workspace(s);
    s.open_picker = Some("option-model".into());
}

fn palette(s: &mut Desktop) {
    workspace(s);
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
    .map(|(conversation, entry, snippet)| SearchHit {
        conversation: ConversationId(conversation),
        entry: EntryId(entry),
        snippet: snippet.into(),
    })
    .into();
    palette::preview(s, "newline", hits);
}

fn diff(s: &mut Desktop) {
    workspace(s);
    changes::preview(s);
}

fn commit(s: &mut Desktop) {
    workspace(s);
    changes::preview_commit(s);
}

fn workspace_open(s: &mut Desktop) {
    workspace(s);
    let id = 0;
    let chat = NewChat::new(id, s.project(), None);
    let previous = s.conversations.iter().find_map(|chat| {
        Some(ManagedWorktree {
            path: chat.worktree.clone()?,
            branch: chat.branch.clone(),
            conversation: chat.id,
            updated_at: chat.updated_at,
        })
    });
    s.selected = None;
    let options = s
        .conversation
        .take()
        .map_or_else(Vec::new, |state| state.options);
    s.thread = Thread::default();
    s.new_chat = Some(chat);
    s.harnesses = vec![Harness {
        name: "omp".into(),
        default: true,
        options,
    }];
    s.set_draft_workspace(
        id,
        Some(Branches {
            current: Some("main".into()),
            default_base: Some("main".into()),
            branches: ["main", "export-v2", "release"].map(str::to_owned).into(),
        }),
        previous.into_iter().collect(),
    );
    s.open_picker = Some("workspace".into());
}

fn workspace_locked(s: &mut Desktop) {
    workspace(s);
    let id = ConversationId(10);
    let mut state = conversation();
    if let (Some(summary), Some(git)) = (s.summary(id), &mut state.git) {
        git.branch.clone_from(&summary.branch);
        git.worktree.clone_from(&summary.worktree);
        state.cwd.clone_from(&summary.worktree);
    }
    s.show(id);
    s.conversation = Some(state);
    s.thread = transcript(&[]);
}

fn workspace_review(s: &mut Desktop) {
    workspace_locked(s);
    changes::preview(s);
}

fn attachments(s: &mut Desktop) {
    workspace(s);
    s.thread = transcript(&["export-before.png", "notes.md"]);
    s.toggle("e4");
    s.attachments = ["Screenshot 2026-11-18.png", "export.log"]
        .map(|name| Attachment {
            name: name.into(),
            mime: "image/png".into(),
            data: String::new(),
        })
        .into();
}

fn empty(s: &mut Desktop) {
    workspace(s);
    s.thread = Thread::default();
}

fn onboarding(s: &mut Desktop) {
    orchestrator(s);
    s.projects.clear();
    s.conversations
        .retain(|chat| chat.kind == ConversationKind::Orchestrator);
}

/// The orchestrator open on its intro.
fn orchestrator(s: &mut Desktop) {
    workspace(s);
    s.show(ConversationId(1));
    let mut state = conversation();
    state.project = None;
    state.git = None;
    state.harness = None;
    state.options.clear();
    s.conversation = Some(state);
}

/// A chat that delegated: a running and a finished subagent, one spawned and
/// one messaged, and the subagent menu open.
fn subagents(s: &mut Desktop) {
    workspace(s);
    s.thread = Thread::from_view(&json!({
        "entries": [
            {"id":1,"kind":"viz.agent.prompt","data":{"text":"Compare the export against the old CLI, and review line endings."}},
            {"id":2,"kind":"viz.agent.text","data":{"text":"I’ll split this: one subagent compares exports, another reviews line endings."}},
            {"id":3,"kind":"viz.agent.tool","data":{
                "id":"spawn-1","title":"Compare exports","kind":"other","status":"in_progress",
                "viz":"spawn_subagent","child":20
            }},
            {"id":4,"kind":"viz.agent.tool","data":{
                "id":"spawn-2","title":"Review line endings","kind":"other","status":"completed",
                "viz":"spawn_subagent","child":21,
                "output":"CRLF survives the join; only the final newline was missing.\nNo other changes needed."
            }},
            {"id":5,"kind":"viz.agent.tool","data":{
                "id":"message-1","title":"Compare exports","kind":"other","status":"completed",
                "viz":"message_subagent","child":20
            }},
        ],
    }));
    s.toggle("e3-group");
    if let Some(state) = &mut s.conversation {
        state.status = ConversationStatus::Working;
    }
}

/// The same chat with its Agents card open.
fn agents(s: &mut Desktop) {
    subagents(s);
    s.toggle("agents-previous");
    s.open_picker = Some("agents".into());
}

/// A subagent's own conversation, under its parent.
fn subagent(s: &mut Desktop) {
    workspace(s);
    s.show(ConversationId(20));
    let mut state = conversation();
    state.status = ConversationStatus::Working;
    s.conversation = Some(state);
    s.thread = Thread::from_view(&json!({
        "entries": [
            {"id":1,"kind":"viz.agent.prompt","data":{"text":"Compare the markdown export with the old CLI’s output for the sample notes."}},
            {"id":2,"kind":"viz.agent.text","data":{"text":"Running both exporters over `samples/` now."}},
        ],
    }));
}

/// The workspace with chats listed under their projects.
fn projects(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_projects(s);
}

/// The delete confirmation for the open chat.
fn delete(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_delete(s, CHAT);
}

fn queue(s: &mut Desktop) {
    workspace(s);
    if let Some(state) = &mut s.conversation {
        state.status = ConversationStatus::Working;
        state.queue = [
            (31, "Also handle CRLF line endings"),
            (32, "Then add a note to the changelog about the export fix"),
        ]
        .map(|(submission, text)| QueuedInput {
            submission: SubmissionId(submission),
            text: text.into(),
        })
        .into();
    }
}

fn option(
    id: &str,
    name: &str,
    category: &str,
    current: &str,
    choices: &[(&str, &str)],
) -> ConfigOption {
    ConfigOption {
        id: id.into(),
        name: name.into(),
        description: None,
        category: Some(category.into()),
        kind: OptionKind::Select,
        value: OptionValue::Select(current.into()),
        choices: choices
            .iter()
            .map(|(value, name)| OptionChoice {
                value: (*value).into(),
                name: (*name).into(),
                description: None,
                group: None,
            })
            .collect(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_millis()).ok())
        .unwrap_or_default()
}
