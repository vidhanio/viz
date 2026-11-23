//! Deterministic `AppSpec` scenes for inspecting desktop layout without a live
//! server.

use viz_client::protocol::{
    Attachment, AttachmentCapabilities, Attention, ConfigOption, DependentChoice, DependentOption,
    Entry, EntryData, EntryId, Environment, EnvironmentStatus, GitBranch, Harness, Lifecycle,
    LiveText, LiveTextKind, LiveTurn, OptionChoice, OptionKind, OptionValue, PlanEntry,
    PlanPriority, PlanStatus, Project, PromptAttachment, RunEnd, SearchHit, Shelf, SlashCommand,
    Task, TaskStatus, ThreadId, ThreadState, ThreadStatus, ThreadSummary, ThreadWorktree,
    ToolCallStatus, ToolDiff, ToolRecord, Worktree, WorktreeTarget,
};

use crate::{
    app::{Checkouts, Desktop, NewThread, Status},
    checkout, composer,
    motion_scene::MotionDemo,
    palette, sidebar,
    thread::Thread,
    timeline,
};

type Scene = (&'static str, fn(&mut Desktop));
pub const SCENES: &[Scene] = &[
    ("workspace", workspace),
    ("motion", motion),
    ("motion-reduced", motion_reduced),
    ("activity", activity),
    ("worklog", worklog),
    ("selectors", selectors),
    ("model-search", model_search),
    ("new-agent", new_agent),
    ("context-menu", context_menu),
    ("palette", palette),
    ("palette-open", palette_open),
    ("attachments", attachments),
    ("slash", slash),
    ("mentions", mentions),
    ("empty", empty),
    ("loading", loading),
    ("onboarding", onboarding),
    ("subagents", subagents),
    ("tasks-expanded", tasks_expanded),
    ("agents", agents),
    ("subagent", subagent),
    ("delete", delete),
    ("overflow", overflow),
    ("projects", projects),
    ("draft-project", draft_project),
    ("draft-worktree", draft_worktree),
    ("move-thread", move_thread),
    ("environments", environments),
    ("offline", offline),
];

const THREAD: ThreadId = ThreadId(11);
const WORKTREE: &str = "/state/worktrees/workbench/viz-trim-export-whitespace";

/// Interactive motion workbench showcasing every primitive in
/// [`crate::motion`].
fn motion(s: &mut Desktop) {
    s.motion_demo = Some(MotionDemo::default());
}

/// The same motion workbench with Reduce Motion forced on for headless proofs.
fn motion_reduced(s: &mut Desktop) {
    s.reduced_motion = Some(true);
    s.motion_demo = Some(MotionDemo::default());
}

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
        git: path == "/work/workbench",
        branch: (path == "/work/workbench").then(|| "main".into()),
    })
    .into();
    s.threads = list();
    s.threads.retain(|thread| thread.parent.is_none());
    s.show(THREAD);
    s.draft.focused = true;
    s.state = Some(thread());
    s.thread = transcript(&[]);
    s.loading = false;
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
        worktree: (id == 10).then(|| ThreadWorktree {
            path: WORKTREE.into(),
            branch: Some("viz/trim-export-whitespace".into()),
        }),
        harness: preview_harness(id).0.into(),
        harness_name: preview_harness(id).1.into(),
        model: match id {
            20 => Some("GPT-5.5".into()),
            21 => Some("Claude Opus 4.7".into()),
            _ => None,
        },
        thinking: parent.is_some().then(|| "high".into()),
        status,
        updated_at: now_ms() - i64::try_from(30 - id).unwrap_or_default() * 60_000,
        lifecycle: Lifecycle {
            unread: id == 9,
            pin: (id == 4).then_some(0),
            ..Lifecycle::default()
        },
        shelf: if id == 4 {
            Shelf::Pinned
        } else {
            Shelf::Active
        },
        attention: (id == 9).then_some(Attention::Unread),
        waiting_on: 0,
        last_input_at: None,
    })
    .into()
}

const fn preview_harness(id: u64) -> (&'static str, &'static str) {
    match id {
        21 | 11 => ("claude", "Claude Code"),
        20 => ("codex", "Codex"),
        9 => ("omp", "Oh My Pi"),
        4 | 10 => ("pi", "Pi"),
        _ => ("agy", "Google Antigravity"),
    }
}

fn thread() -> ThreadState {
    ThreadState {
        status: ThreadStatus::Idle,
        harness: "claude".into(),
        project: Some("/work/workbench".into()),
        worktree: None,
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
                answer: Some(
                    "The export drops the final newline when it joins blocks. I’ll fix that at the boundary."
                        .into(),
                ),
            }),
        ],
        Some(EntryId(1)),
    )
}

/// An expanded work log with an empty search, padded reasoning and terminal
/// output.
fn activity(s: &mut Desktop) {
    workspace(s);
    s.thread = preview_thread(
        [
            prompt("Check the sample exports."),
            EntryData::Tool(ToolRecord {
                input: Some(r#"{"query":"export newline conventions"}"#.into()),
                ..preview_tool("search-1", "Search", "search", ToolCallStatus::Completed)
            }),
            EntryData::Thought {
                key: "1:0".into(),
                text: "\n\n**Compare the output.** The only meaningful difference should be the final newline.\n\n\n\n".into(),
            },
            EntryData::Tool(ToolRecord {
                input: Some(r#"{"command":"wc -l old.md new.md"}"#.into()),
                output: Some(
                    "Created At: 2026-11-20T19:51:26-05:00\nCompleted At: 2026-11-20T19:51:31-05:00\nCommand exited with code 0.\n\nCommand output:\n    42 old.md\n    43 new.md\n    85 total\n".into(),
                ),
                ..preview_tool("run-1", "Check line counts", "execute", ToolCallStatus::Completed)
            }),
            response("The updated export preserves the final newline."),
        ],
        None,
    );
    s.toggle("acp-1-search-1-activity");
    s.toggle("acp-1-run-1");
}

/// Every kind of activity, open: a plan, reads, an edit's diff, a failed
/// command, a tool viz doesn't know, a delegated task's result and a stopped
/// run.
fn worklog(s: &mut Desktop) {
    workspace(s);
    let tool = |id: &str, title: &str, kind: &str, input: &str| ToolRecord {
        input: Some(input.into()),
        ..preview_tool(id, title, kind, ToolCallStatus::Completed)
    };
    s.thread = preview_thread(
        [
            prompt("Make the export keep CRLF line endings, then run the tests."),
            EntryData::Plan {
                entries: [
                    ("Find where blocks are joined", PlanStatus::Completed),
                    ("Keep the input's line endings", PlanStatus::InProgress),
                    ("Run the export tests", PlanStatus::Pending),
                ]
                .into_iter()
                .map(|(content, status)| PlanEntry {
                    content: content.into(),
                    priority: PlanPriority::Medium,
                    status,
                })
                .collect(),
            },
            EntryData::Thought {
                key: "1:0".into(),
                text: "The join uses a literal `\\n`; it should reuse whatever the source file used."
                    .into(),
            },
            EntryData::Tool(tool(
                "read-1",
                "Read",
                "read",
                r#"{"path":"/work/workbench/src/export/markdown.rs"}"#,
            )),
            EntryData::Tool(tool("grep-1", "Search", "search", r#"{"pattern":"join(\"\\n"}"#)),
            EntryData::Tool(ToolRecord {
                output: Some("Updated 1 file.".into()),
                diffs: vec![ToolDiff {
                    path: "/work/workbench/src/export/markdown.rs".into(),
                    old_text: Some(
                        "fn render(blocks: &[Block]) -> String {\n    let body = blocks.join(\"\\n\\n\");\n    body\n}\n"
                            .into(),
                    ),
                    new_text: "fn render(blocks: &[Block], ending: LineEnding) -> String {\n    let separator = ending.repeat(2);\n    let body = blocks.join(&separator);\n    body + ending.as_str()\n}\n"
                        .into(),
                }],
                ..tool(
                    "edit-1",
                    "Edit",
                    "edit",
                    r#"{"path":"/work/workbench/src/export/markdown.rs"}"#,
                )
            }),
            EntryData::Tool(ToolRecord {
                output: Some(
                    "Created At: 2026-11-20T19:51:26-05:00\nCompleted At: 2026-11-20T19:51:31-05:00\nCommand exited with code 101.\n\nCommand output:\nrunning 12 tests\ntest export::crlf ... FAILED\n\nfailures:\n    export::crlf: expected \"a\\r\\n\\r\\nb\\r\\n\", got \"a\\r\\n\\r\\nb\\n\"\n".into(),
                ),
                ..tool(
                    "test-1",
                    "Run tests",
                    "execute",
                    r#"{"command":"cargo test -p workbench export::"}"#,
                )
            }),
            EntryData::Tool(tool(
                "mcp-1",
                "mcp__docs__search_pages",
                "mcp__docs__search_pages",
                r#"{"query":"CRLF in CommonMark"}"#,
            )),
            response("The test caught the last block's ending; I’ll fix that next."),
            EntryData::TaskResult {
                task: "t-21".into(),
                text: "Task t-21 (Review line endings) on local thread 21 completed.\n\nCRLF survives the join; only the final newline was missing.".into(),
            },
            EntryData::Run(RunEnd {
                stop_reason: Some("cancelled".into()),
                error: None,
                answer: None,
            }),
        ],
        None,
    );
    for key in ["1:0-activity", "acp-1-edit-1", "acp-1-test-1", "e10"] {
        s.toggle(key);
    }
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

/// A fresh thread in a project with the harness and model picker side by side.
fn new_agent(s: &mut Desktop) {
    workspace(s);
    let thread = NewThread::new(0, s.project());
    s.selected = None;
    s.state = None;
    s.thread = Thread::default();
    s.new_thread = Some(thread);
    s.harnesses = vec![
        Harness {
            id: "omp".into(),
            name: "Oh My Pi".into(),
            default: false,
            options: options(),
        },
        Harness {
            id: "codex".into(),
            name: "Codex".into(),
            default: false,
            options: Vec::new(),
        },
        Harness {
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
                ("max", "max"),
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

fn palette_open(s: &mut Desktop) {
    workspace(s);
    palette::preview(s, "", Vec::new());
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

/// A thread just selected whose transcript hasn't arrived.
fn loading(s: &mut Desktop) {
    workspace(s);
    s.show(ThreadId(10));
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
        result: (status == TaskStatus::Completed)
            .then(|| "CRLF survives the join; only the final newline was missing.".into()),
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

/// The persistent Tasks Dock expanded above the composer, including earlier
/// settled tasks folded under a disclosure row.
fn tasks_expanded(s: &mut Desktop) {
    subagents(s);
    s.tasks.push(Task {
        id: "22".into(),
        parent: THREAD,
        environment: "build-box".into(),
        thread: Some(ThreadId(22)),
        run: None,
        status: TaskStatus::Completed,
        reachable: true,
        title: "Verify Linux release build".into(),
        result: Some("Release binary built cleanly.".into()),
        error: None,
    });
    s.tasks.push(Task {
        id: "23".into(),
        parent: THREAD,
        environment: "local".into(),
        thread: Some(ThreadId(23)),
        run: None,
        status: TaskStatus::Completed,
        reachable: true,
        title: "Check markdown table escaping".into(),
        result: Some("Pipe escapes verified.".into()),
        error: None,
    });
    s.toggle(composer::TASKS_DOCK_KEY);
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
    s.loading = false;
    let mut state = thread();
    state.status = ThreadStatus::Working;
    state.usage = Some(viz_client::protocol::ContextUsage {
        used: 34_817,
        size: 1_000_000,
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

/// The workspace with threads listed under their projects (including a
/// collapsed project).
fn projects(s: &mut Desktop) {
    workspace(s);
    s.projects.push(Project {
        name: "Release scripts".into(),
        path: "/work/release-scripts".into(),
        created_at: 0,
        git: false,
        branch: None,
    });
    sidebar::preview_projects(s, &["/work/release-scripts"]);
}

/// A fresh thread with its project picker open.
fn draft_project(s: &mut Desktop) {
    new_agent(s);
    s.open_picker = Some(timeline::DRAFT_PROJECT.into());
}

/// The Workbench project's preview branches and worktrees.
fn checkouts(s: &mut Desktop) {
    s.checkouts.insert(
        "/work/workbench".into(),
        Checkouts {
            branches: ["main", "release", "origin/main", "origin/release"]
                .map(|name| GitBranch {
                    name: name.into(),
                    remote: name.starts_with("origin/"),
                })
                .into(),
            worktrees: vec![
                Worktree {
                    path: "/work/workbench".into(),
                    branch: Some("main".into()),
                    main: true,
                    managed: false,
                    threads: vec![THREAD],
                },
                Worktree {
                    path: WORKTREE.into(),
                    branch: Some("viz/trim-export-whitespace".into()),
                    main: false,
                    managed: true,
                    threads: vec![ThreadId(10)],
                },
            ],
        },
    );
}

/// A fresh thread in a Git project with its checkout picker open.
fn draft_worktree(s: &mut Desktop) {
    new_agent(s);
    checkouts(s);
    if let Some(draft) = &mut s.new_thread {
        draft.worktree = Some(WorktreeTarget::New {
            base: None,
            branch: None,
        });
    }
    s.open_picker = Some(checkout::DRAFT_CHECKOUT.into());
}

/// A thread in a worktree with the toolbar's move menu open.
fn move_thread(s: &mut Desktop) {
    workspace(s);
    checkouts(s);
    s.show(ThreadId(10));
    s.thread = transcript(&[]);
    s.loading = false;
    s.open_picker = Some(checkout::THREAD_CHECKOUT.into());
}

/// Multiple configured environments with the sidebar footer picker open.
fn environments(s: &mut Desktop) {
    workspace(s);
    s.environments = vec![
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
    sidebar::preview_environment_menu(s);
}

/// Offline state showing the connection error and endpoint without a duplicate
/// retry button in the sidebar footer.
fn offline(s: &mut Desktop) {
    s.status = Status::Error(
        "connection refused (os error 111): could not reach viz-server at ws://127.0.0.1:7420/ws"
            .into(),
    );
    s.endpoint = Some("http://127.0.0.1:7420".into());
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

fn slash(s: &mut Desktop) {
    workspace(s);
    if let Some(state) = &mut s.state {
        state.commands = vec![
            SlashCommand {
                name: "compact".into(),
                description: "Summarize the conversation to free context window".into(),
                hint: Some("[focus]".into()),
            },
            SlashCommand {
                name: "review".into(),
                description: "Review unstaged changes in the working tree".into(),
                hint: Some("[path]".into()),
            },
            SlashCommand {
                name: "plan".into(),
                description: "Draft a step-by-step plan before making edits".into(),
                hint: None,
            },
        ];
    }
    s.draft = rune::TextEdit::new("/");
    s.draft.focused = true;
    s.completion.preview_slash("", 0);
}

fn mentions(s: &mut Desktop) {
    workspace(s);
    s.draft = rune::TextEdit::new("@export");
    s.draft.focused = true;
    s.completion.preview_files(
        "export",
        vec![
            "src/export/markdown.rs".into(),
            "src/export/line_ending.rs".into(),
            "crates/workbench/src/export/mod.rs".into(),
        ],
        1,
    );
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
