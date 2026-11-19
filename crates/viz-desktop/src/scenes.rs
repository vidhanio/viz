//! Deterministic `AppSpec` scenes for inspecting desktop layout without a live
//! socket.

use serde_json::json;
use viz_client::protocol::{
    Approval, ApprovalKind, ApprovalOption, Attachment, AttachmentCapabilities, ConfigOption,
    ConversationId, ConversationKind, ConversationState, ConversationStatus, ConversationSummary,
    EntryId, GitFile, GitFileStatus, GitState, OptionChoice, OptionKind, OptionValue, Project,
    QueuedInput, SearchHit, SlashCommand, SubmissionId,
};

use crate::{
    app::{Desktop, Status},
    changes, palette, sidebar,
    thread::Thread,
};

type Scene = (&'static str, fn(&mut Desktop));
pub const SCENES: &[Scene] = &[
    ("workspace", workspace),
    ("selectors", selectors),
    ("approval", approval),
    ("palette", palette),
    ("diff", diff),
    ("commit", commit),
    ("worktree", worktree),
    ("attachments", attachments),
    ("queue", queue),
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
            title: Some(title.into()),
            kind,
            parent: parent.map(ConversationId),
            project: project.map(str::to_owned),
            harness: (kind != Orchestrator).then(|| "omp".into()),
            status,
            archived: false,
            updated_at: i64::try_from(id).unwrap_or_default(),
            unread: id == 9,
            pinned: id == 4,
            worktree: (id == 10).then(|| "/work/workbench/.viz/worktrees/trim-export".into()),
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
            option(
                "access",
                "Access",
                "access",
                "ask",
                &[
                    ("full", "Full access"),
                    ("ask", "Ask first"),
                    ("read-only", "Read-only"),
                ],
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
        approvals: Vec::new(),
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

fn approval(s: &mut Desktop) {
    workspace(s);
    s.toggle("e4");
    if let Some(state) = &mut s.conversation {
        state.status = ConversationStatus::Approval;
        state.approvals.push(Approval {
            id: "run-export".into(),
            title: "Run the markdown export".into(),
            kind: Some("execute".into()),
            detail: Some("{\n  \"command\": \"cargo run -- export notes/\"\n}".into()),
            options: [
                ("reject", "Deny", ApprovalKind::RejectOnce),
                ("always", "Always allow", ApprovalKind::AllowAlways),
                ("allow", "Allow once", ApprovalKind::AllowOnce),
            ]
            .map(|(id, name, kind)| ApprovalOption {
                id: id.into(),
                name: name.into(),
                kind,
            })
            .into(),
        });
    }
    if let Some(chat) = s.conversations.iter_mut().find(|chat| chat.id == CHAT) {
        chat.status = ConversationStatus::Approval;
    }
    s.toggle("approval-run-export");
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

fn worktree(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_worktree(s);
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
