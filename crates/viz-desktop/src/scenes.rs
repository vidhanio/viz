//! Deterministic `AppSpec` scenes for inspecting desktop layout without a live
//! backend.

use viz_client::protocol::{
    Attachment, AttachmentCapabilities, ConfigOption, DependentChoice, DependentOption, Entry,
    EntryData, EntryId, Environment, EnvironmentStatus, Harness, LiveText, LiveTextKind, LiveTurn,
    OptionChoice, OptionKind, OptionValue, PlanEntry, PlanPriority, PlanStatus, Project,
    PromptAttachment, QueuedInput, RunEnd, RunId, SearchHit, SlashCommand, Task, TaskStatus,
    ThreadId, ThreadState, ThreadStatus, ThreadSummary, ToolCallStatus, ToolDiff, ToolRecord,
};

use crate::{
    app::{Desktop, NewThread, Status},
    composer, palette, sidebar,
    thread::Thread,
    timeline,
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
    ("subagents", subagents),
    ("agents", agents),
    ("subagent", subagent),
    ("delete", delete),
    ("overflow", overflow),
    ("projects", projects),
    ("draft-project", draft_project),
];

const THREAD: ThreadId = ThreadId(11);

fn workspace(s: &mut Desktop) {
    s.status = Status::Connected {
        version: "preview".into(),
    };
    s.environment = Some("local".into());
    s.environments = vec![Environment {
        id: "local".into(),
        url: None,
        status: EnvironmentStatus::Local,
    }];
    s.projects = [
        ("Workbench", "/work/workbench"),
        ("Field notes", "/work/field-notes"),
    ]
    .map(|(name, path)| Project {
        name: name.into(),
        path: path.into(),
        created_at: 0,
    })
    .into();
    s.threads = list();
    s.threads.retain(|thread| thread.parent.is_none());
    s.show(THREAD);
    s.draft.focused = true;
    s.state = Some(thread());
    s.thread = transcript(&[]);
    s.toggle("acp-1-write-1-activity");
    s.toggle("acp-1-write-1");
}

fn list() -> Vec<ThreadSummary> {
    use ThreadStatus::{Failed, Idle, Working};

    [
        (
            21,
            "Review line endings",
            Some(11),
            Some("/work/workbench"),
            Idle,
        ),
        (
            20,
            "Compare exports",
            Some(11),
            Some("/work/workbench"),
            Working,
        ),
        (
            11,
            "Export markdown notes",
            None,
            Some("/work/workbench"),
            Idle,
        ),
        (
            10,
            "Trim export whitespace",
            None,
            Some("/work/workbench"),
            Idle,
        ),
        (
            9,
            "Audit error messages",
            None,
            Some("/work/workbench"),
            Idle,
        ),
        (
            7,
            "Organize reference links",
            None,
            Some("/work/field-notes"),
            Failed,
        ),
        (4, "Draft release notes", None, None, Idle),
    ]
    .map(|(id, title, parent, project, status)| ThreadSummary {
        id: ThreadId(id),
        title: title.into(),
        parent: parent.map(ThreadId),
        parent_environment: None,
        project: project.map(str::to_owned),
        // Every known mark among the threads.
        harness: {
            let marks = ["claude", "codex", "goose", "omp", "pi", "claude"];
            marks[usize::try_from(id).unwrap_or_default() % marks.len()].into()
        },
        model: match id {
            20 => Some("GPT-5.5".into()),
            21 => Some("Claude Opus 4.7".into()),
            _ => None,
        },
        thinking: parent.is_some().then(|| "high".into()),
        status,
        archived: false,
        // Minutes ago, newest first by id, so "last activity" reads
        // like a live session.
        updated_at: now_ms() - i64::try_from(30 - id).unwrap_or_default() * 60_000,
        unread: id == 9,
        pinned: id == 4,
    })
    .into()
}

fn thread() -> ThreadState {
    ThreadState {
        status: ThreadStatus::Idle,
        harness: "claude".into(),
        project: Some("/work/workbench".into()),
        cwd: "/work/workbench".into(),
        options: options(),
        commands: vec![SlashCommand {
            name: "review".into(),
            description: "Review the current changes".into(),
            hint: Some("[path]".into()),
        }],
        queue: Vec::new(),
        attachments: AttachmentCapabilities { images: true },
        live: None,
        usage: None,
    }
}

fn transcript(attachments: &[&str]) -> Thread {
    let attachments = attachments
        .iter()
        .map(|&name| PromptAttachment {
            id: name.to_owned(),
            name: name.to_owned(),
            mime: "image/png".into(),
        })
        .collect();
    preview_thread(
        [
            EntryData::Prompt {
                text: "Make the markdown export preserve a final newline.".into(),
                attachments,
                origin: None,
            },
            response(
                "The export drops the final newline when it joins blocks. I’ll fix that at the boundary.",
            ),
            EntryData::Plan {
                entries: [
                    ("Read the export pipeline", PlanStatus::Completed),
                    ("Preserve the final newline", PlanStatus::Completed),
                    ("Check export output", PlanStatus::InProgress),
                ]
                .into_iter()
                .map(|(content, status)| PlanEntry {
                    content: content.into(),
                    priority: PlanPriority::Medium,
                    status,
                })
                .collect(),
            },
            EntryData::Tool(ToolRecord {
                output: Some("Updated 1 file.".into()),
                diffs: vec![ToolDiff {
                    path: "src/export.rs".into(),
                    old_text: Some("let body = blocks.join(\"\n\n\");\nOk(body)\n".into()),
                    new_text: "let body = blocks.join(\"\n\n\");\nOk(format!(\"{body}\n\"))\n"
                        .into(),
                }],
                ..preview_tool(
                    "write-1",
                    "Update markdown export",
                    "edit",
                    ToolCallStatus::Completed,
                )
            }),
            EntryData::Run(RunEnd {
                stop_reason: Some("end_turn".into()),
                error: None,
                answer: None,
            }),
        ],
        Some(EntryId(1)),
    )
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

/// A fresh thread in a project with two runners: the harness and the model
/// picker side by side.
fn new_agent(s: &mut Desktop) {
    workspace(s);
    let thread = NewThread::new(0, s.project());
    s.selected = None;
    s.state = None;
    s.thread = Thread::default();
    s.new_thread = Some(thread);
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

/// A thread's context menu with its Move to project submenu open.
fn context_menu(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_context_menu(s, ThreadId(10), 120.0, 236.0);
}

/// A runner's catalog as omp reports it: models from several providers
/// (two sharing a name), thinking levels for the current model and those it
/// reported for others, and an uncategorized switch.
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
    .map(|(thread, entry, snippet)| SearchHit {
        thread: ThreadId(thread),
        entry: EntryId(entry),
        snippet: snippet.into(),
    })
    .into();
    palette::preview(s, "newline", hits);
}

fn attachments(s: &mut Desktop) {
    workspace(s);
    s.thread = transcript(&["export-before.png", "notes.md"]);
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
    workspace(s);
    s.projects.clear();
    s.threads.clear();
    s.selected = None;
    s.state = None;
    s.thread = Thread::default();
}

/// A thread that delegated to a running and a finished subagent.
fn subagents(s: &mut Desktop) {
    workspace(s);
    s.threads = list();
    s.tasks = [
        (20, "Compare exports", TaskStatus::Running),
        (21, "Review line endings", TaskStatus::Completed),
    ]
    .map(|(id, title, status)| Task {
        id: id.to_string(),
        parent: THREAD,
        environment: "local".into(),
        thread: Some(ThreadId(id)),
        run: None,
        status,
        reachable: true,
        title: title.into(),
        result: None,
        error: None,
    })
    .into();
    s.thread = preview_thread([
        prompt("Compare the export against the old CLI, and review line endings."),
        response("I’ll delegate the export comparison and the line-ending review."),
        EntryData::Tool(ToolRecord {
            viz: Some("delegate_task".into()), child: Some(ThreadId(20)),
            ..preview_tool("spawn-1", "Compare exports", "other", ToolCallStatus::InProgress)
        }),
        EntryData::Tool(ToolRecord {
            viz: Some("delegate_task".into()), child: Some(ThreadId(21)),
            output: Some("CRLF survives the join; only the final newline was missing.\nNo other changes needed.".into()),
            ..preview_tool("spawn-2", "Review line endings", "other", ToolCallStatus::Completed)
        }),
        EntryData::Tool(ToolRecord {
            viz: Some("thread_send".into()), child: Some(ThreadId(20)),
            ..preview_tool("message-1", "Compare exports", "other", ToolCallStatus::Completed)
        }),
    ], None);
    if let Some(state) = &mut s.state {
        state.status = ThreadStatus::Working;
    }
}

/// The same thread with its timeline activity expanded.
fn agents(s: &mut Desktop) {
    subagents(s);
    s.toggle("acp-1-spawn-1-activity");
}

/// A subagent's own thread, under its parent.
fn subagent(s: &mut Desktop) {
    workspace(s);
    s.threads = list();
    s.show(ThreadId(20));
    let mut state = thread();
    state.status = ThreadStatus::Working;
    state.usage = Some(viz_client::protocol::ContextUsage {
        used: 34_817,
        size: 200_000,
    });
    state.live = Some(LiveTurn {
        turn: "1".into(),
        text: Some(LiveText {
            key: "1:1".into(),
            kind: LiveTextKind::Thought,
            text: "Run both exporters against the same inputs and compare their output.".into(),
        }),
        tools: vec![ToolRecord {
            started_at: Some(now_ms() - 11_000),
            input: Some(r#"{"command":"diff -u old.md new.md","timeout":30}"#.into()),
            ..preview_tool(
                "compare",
                "Compare sample exports",
                "execute",
                ToolCallStatus::InProgress,
            )
        }],
    });
    s.thread = preview_thread(
        [
            prompt("Compare the markdown export with the old CLI’s output for the sample notes."),
            response("I’ll compare the output for `samples/`."),
        ],
        None,
    );
    s.thread.sync_acp(state.live.as_ref());
    s.state = Some(state);
}

/// The workspace with threads listed under their projects.
fn projects(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_projects(s);
}

/// A fresh thread with its project picker open.
fn draft_project(s: &mut Desktop) {
    new_agent(s);
    s.open_picker = Some(timeline::DRAFT_PROJECT.into());
}

/// The delete confirmation for the open thread.
fn delete(s: &mut Desktop) {
    workspace(s);
    sidebar::preview_delete(s, THREAD);
}

/// Long strings in the delete dialog and the toasts: titles wrap or cut
/// before the close buttons, details stop at their line caps.
fn overflow(s: &mut Desktop) {
    workspace(s);
    if let Some(thread) = s.threads.iter_mut().find(|thread| thread.id == THREAD) {
        thread.title = "Which Rust edition does /home/vidhanio/Projects/viz/Cargo.toml use, and \
                      does every crate in the workspace inherit it?"
            .into();
    }
    sidebar::preview_delete(s, THREAD);
    s.preview_toast(
        "Couldn't open /home/vidhanio/Projects/viz/crates/viz-desktop/src/sidebar/actions.rs \
         for writing",
        "error: failed to write \
         /home/vidhanio/Projects/viz/target/preview/export-markdown-notes-4f1c2a/crates/\
         viz-desktop/src/ui/mod.rs: Permission denied (os error 13)\nhint: check the \
         directory's owner\nhint: or remove it and start the thread again\nnote: the backend \
         kept the previous version on disk",
    );
    s.preview_toast("Offline", "The backend disconnected.");
}

fn queue(s: &mut Desktop) {
    workspace(s);
    if let Some(state) = &mut s.state {
        state.status = ThreadStatus::Working;
        state.queue = [
            (31, "Also handle CRLF line endings"),
            (32, "Then add a note to the changelog about the export fix"),
        ]
        .map(|(run, text)| QueuedInput {
            run: RunId(run),
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

fn preview_thread(entries: impl IntoIterator<Item = EntryData>, before: Option<EntryId>) -> Thread {
    let entries = (1_u64..)
        .zip(entries)
        .map(|(id, data)| Entry {
            id: EntryId(id),
            data,
        })
        .collect::<Vec<_>>();
    Thread::from_entries(&entries, before)
}

fn prompt(text: &str) -> EntryData {
    EntryData::Prompt {
        text: text.into(),
        attachments: Vec::new(),
        origin: None,
    }
}

fn response(text: &str) -> EntryData {
    EntryData::Text {
        text: text.into(),
        key: "1:0".into(),
    }
}

fn preview_tool(id: &str, title: &str, kind: &str, status: ToolCallStatus) -> ToolRecord {
    ToolRecord {
        id: id.into(),
        title: title.into(),
        name: None,
        kind: Some(kind.into()),
        status,
        viz: None,
        child: None,
        child_environment: None,
        started_at: None,
        input: None,
        output: None,
        diffs: Vec::new(),
        locations: Vec::new(),
    }
}
