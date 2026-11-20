//! Deterministic `AppSpec` scenes for inspecting desktop layout without a live
//! socket.

use serde_json::json;
use viz_client::protocol::{
    Attachment, AttachmentCapabilities, ConfigOption, ConversationId, ConversationKind,
    ConversationState, ConversationStatus, ConversationSummary, DependentChoice, DependentOption,
    EntryId, Harness, OptionChoice, OptionKind, OptionValue, Project, QueuedInput, SearchHit,
    SlashCommand, SubmissionId,
};

use crate::{
    app::{Desktop, NewChat, Status},
    composer, palette, sidebar,
    thread::Thread,
};

type Scene = (&'static str, fn(&mut Desktop));
pub const SCENES: &[Scene] = &[
    ("workspace", workspace),
    ("selectors", selectors),
    ("model-search", model_search),
    ("new-agent", new_agent),
    ("context-menu", context_menu),
    ("palette", palette),
    ("attachments", attachments),
    ("queue", queue),
    ("empty", empty),
    ("onboarding", onboarding),
    ("orchestrator", orchestrator),
    ("subagents", subagents),
    ("agents", agents),
    ("subagent", subagent),
    ("delete", delete),
    ("overflow", overflow),
    ("projects", projects),
];

const CHAT: ConversationId = ConversationId(11);

fn workspace(s: &mut Desktop) {
    s.status = Status::Connected {
        version: "preview".into(),
    };
    s.root = Some(ConversationId(1));
    s.projects = [
        ("workbench", "Workbench", "/work/workbench"),
        ("field-notes", "Field notes", "/work/field-notes"),
    ]
    .map(|(id, name, path)| Project {
        id: id.into(),
        name: name.into(),
        path: path.into(),
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
            // Every known mark among the chats.
            harness: (kind != Orchestrator).then(|| {
                let marks = ["claude", "codex", "goose", "omp", "pi", "claude"];
                marks[usize::try_from(id).unwrap_or_default() % marks.len()].into()
            }),
            status,
            archived: false,
            // Minutes ago, newest first by id, so "last activity" reads
            // like a live session.
            updated_at: now_ms() - i64::try_from(30 - id).unwrap_or_default() * 60_000,
            unread: id == 9,
            pinned: id == 4,
        },
    )
    .into()
}

fn conversation() -> ConversationState {
    ConversationState {
        status: ConversationStatus::Idle,
        harness: Some("claude".into()),
        project: Some("workbench".into()),
        cwd: Some("/work/workbench".into()),
        options: options(),
        commands: vec![SlashCommand {
            name: "review".into(),
            description: "Review the current changes".into(),
            hint: Some("[path]".into()),
        }],
        queue: Vec::new(),
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

/// The model picker open on the session's model.
fn selectors(s: &mut Desktop) {
    workspace(s);
    composer::preview_model_menu(s, "");
}

/// The model picker with a search typed: two providers' matches.
fn model_search(s: &mut Desktop) {
    workspace(s);
    composer::preview_model_menu(s, "fl");
}

/// A fresh chat in a project with two runners: the harness and the model
/// picker side by side.
fn new_agent(s: &mut Desktop) {
    workspace(s);
    let chat = NewChat::new(0, s.project());
    s.selected = None;
    s.conversation = None;
    s.thread = Thread::default();
    s.new_chat = Some(chat);
    s.harnesses = vec![
        Harness {
            name: "omp".into(),
            default: true,
            options: options(),
        },
        Harness {
            name: "pi".into(),
            default: false,
            options: Vec::new(),
        },
    ];
}

/// A chat's context menu with its Move to project submenu open.
fn context_menu(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_context_menu(s, ConversationId(10), 120.0, 236.0);
}

/// A runner's catalog as omp reports it: models from several providers
/// (two sharing a name), thinking levels for the current model and those it
/// reported for others, a real mode, and an uncategorized switch.
fn options() -> Vec<ConfigOption> {
    let levels = |levels: &[(&str, &str)]| DependentOption {
        id: "thinking".into(),
        name: "Thinking".into(),
        description: None,
        value: OptionValue::Select(levels[0].0.into()),
        choices: levels
            .iter()
            .map(|(value, name)| DependentChoice {
                value: (*value).into(),
                name: (*name).into(),
                description: None,
            })
            .collect(),
    };
    let mut model = option(
        "model",
        "Model",
        "model",
        "anthropic/claude-opus-5-5",
        &[
            ("anthropic/claude-fable-5", "Claude Fable 5"),
            ("anthropic/claude-haiku-4-5", "Claude Haiku 4.5"),
            ("anthropic/claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
            ("anthropic/claude-opus-5-5", "Claude Opus 5.5"),
            ("anthropic/claude-sonnet-5", "Claude Sonnet 5"),
            ("openai/gpt-6", "GPT-6"),
            ("openai/gpt-6-mini", "GPT-6 mini"),
            ("google/gemini-3-pro", "Gemini 3 Pro"),
            ("google/gemini-3-flash", "Gemini 3 Flash"),
            ("openrouter/moonshotai/kimi-k3", "Kimi K3"),
        ],
    );
    for choice in &mut model.choices {
        choice.thinking = match choice.value.as_str() {
            "anthropic/claude-haiku-4-5" => Some(levels(&[
                ("off", "Off"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
            ])),
            "openai/gpt-6-mini" => Some(levels(&[("off", "Off")])),
            _ => None,
        };
    }
    vec![
        model,
        option(
            "thinking",
            "Thinking",
            "thought_level",
            "high",
            &[
                ("off", "Off"),
                ("low", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
            ],
        ),
        option(
            "mode",
            "Mode",
            "mode",
            "default",
            &[("default", "Default"), ("plan", "Plan")],
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
    ]
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

/// Long strings in the delete dialog and the toasts: titles wrap or cut
/// before the close buttons, details stop at their line caps.
fn overflow(s: &mut Desktop) {
    workspace(s);
    if let Some(chat) = s.conversations.iter_mut().find(|chat| chat.id == CHAT) {
        chat.title = "Which Rust edition does /home/vidhanio/Projects/viz/Cargo.toml use, and \
                      does every crate in the workspace inherit it?"
            .into();
    }
    sidebar::preview_delete(s, CHAT);
    s.preview_toast(
        "Couldn't open /home/vidhanio/Projects/viz/crates/viz-desktop/src/sidebar/actions.rs \
         for writing",
        "error: failed to write \
         /home/vidhanio/Projects/viz/target/preview/export-markdown-notes-4f1c2a/crates/\
         viz-desktop/src/ui/mod.rs: Permission denied (os error 13)\nhint: check the \
         directory's owner\nhint: or remove it and start the chat again\nnote: the backend \
         kept the previous version on disk",
    );
    s.preview_toast("Offline", "Backend disconnected");
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
                thinking: None,
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
