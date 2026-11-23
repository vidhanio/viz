//! Pure [`Model`] fixture builders for timeline, workspace, and subagent
//! scenes.

use viz_client::protocol::{
    AttachmentCapabilities, Attention, Body, ConfigOption, ContextUsage, DependentChoice,
    DependentOption, Entry, EntryData, EntryId, Environment, EnvironmentStatus, GitBranch,
    Lifecycle, LiveTurn, OptionChoice, OptionKind, OptionValue, Plan, PlanContent, PlanEntry,
    PlanPriority, PlanStatus, Project, PromptAttachment, RunEnd, Segment, Shelf, SlashCommand,
    Task, TaskStatus, ThreadId, ThreadState, ThreadStatus, ThreadSummary, ThreadWorktree, ToolCall,
    ToolCallStatus, ToolContent, ToolDiff, TranscriptEntry, VizLink, Worktree,
};

use crate::{
    composer,
    model::{Checkouts, Model, PaneSwitch, Status, Transcript},
};

pub(super) const THREAD: ThreadId = ThreadId(11);
pub(super) const WORKTREE: &str = "/state/worktrees/workbench/viz-trim-export-whitespace";

/// Populates the baseline connected workspace model.
pub(super) fn seed_workspace(m: &mut Model) {
    m.connection.status = Status::Connected {
        version: "preview".into(),
    };
    m.connection.environment = Some("local".into());
    m.connection.environments = vec![Environment {
        id: "local".into(),
        url: None,
        status: EnvironmentStatus::Local,
    }];
    let projects: Vec<Project> = [
        ("Workbench", "/work/workbench"),
        ("Field notes", "/work/field-notes"),
    ]
    .map(|(name, path)| Project {
        settings: viz_client::protocol::ProjectSettings::default(),
        name: name.into(),
        path: path.into(),
        created_at: 0,
        git: path == "/work/workbench",
        branch: (path == "/work/workbench").then(|| "main".into()),
    })
    .into();
    let cache = m.cache_mut();
    cache.set_projects(projects);
    cache.set_threads(list());
    cache.retain_threads(|thread| thread.parent.is_none());
    select_preview(m, THREAD);
    m.composer.draft_focused = true;
    let open = m.cache_mut().open_thread_mut(THREAD);
    open.state = Some(thread_state());
    open.transcript = transcript(&[]);
    open.loading = false;
    toggle(m, "acp-1-write-1-activity");
    toggle(m, "acp-1-write-1");
}

/// Switches selection in a preview model, mirroring `Desktop::show` semantics.
pub(super) fn select_preview(m: &mut Model, thread: ThreadId) {
    if m.selection.selected != Some(thread) || m.selection.new_thread.is_some() {
        m.selection.switch = if m.selection.new_thread.is_some() {
            PaneSwitch::Fade
        } else {
            m.selection
                .selected
                .map_or(PaneSwitch::Cut, |from| m.switch_direction(from, thread))
        };
        m.selection.new_thread = None;
        m.selection.selected = Some(thread);
        m.composer.draft_focused = false;
        m.disclosure.expanded.clear();
        m.composer.completion.reset();
        m.composer.open_picker = None;
        let open = m.cache_mut().open_thread_mut(thread);
        open.transcript = Transcript::default();
        open.state = None;
        open.loading = true;
        open.history_loading = false;
    }
}

pub(super) fn toggle(m: &mut Model, key: &str) {
    if !m.disclosure.expanded.remove(key) {
        m.disclosure.expanded.insert(key.to_owned());
    }
}

pub(super) fn seed_activity(m: &mut Model) {
    seed_workspace(m);
    m.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(
        [
            prompt("Check the sample exports."),
            preview_tool_entry(ToolCall {
                raw_input: Some(r#"{"query":"export newline conventions"}"#.into()),
                ..preview_tool("search-1", "Search", "search", ToolCallStatus::Completed)
            }),
            EntryData::Transcript(preview_text(
                "1:0",
                "\n\n**Compare the output.** The only meaningful difference should be the final newline.\n\n\n\n",
                true,
            )),
            preview_tool_entry(ToolCall {
                raw_input: Some(r#"{"command":"wc -l old.md new.md"}"#.into()),
                raw_output: Some(
                    "Created At: 2026-11-20T19:51:26-05:00\nCompleted At: 2026-11-20T19:51:31-05:00\nCommand exited with code 0.\n\nCommand output:\n    42 old.md\n    43 new.md\n    85 total\n".into(),
                ),
                ..preview_tool("run-1", "Check line counts", "execute", ToolCallStatus::Completed)
            }),
            response("The updated export preserves the final newline."),
        ],
        None,
    );
    toggle(m, "acp-1-search-1-activity");
    toggle(m, "acp-1-run-1");
}

pub(super) fn seed_worklog(m: &mut Model) {
    seed_workspace(m);
    let tool = |id: &str, title: &str, kind: &str, input: &str| ToolCall {
        raw_input: Some(input.into()),
        ..preview_tool(id, title, kind, ToolCallStatus::Completed)
    };
    m.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(
        [
            prompt("Make the export keep CRLF line endings, then run the tests."),
            preview_plan(
                [
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
            ),
            EntryData::Transcript(preview_text(
                "1:0",
                "The join uses a literal `\\n`; it should reuse whatever the source file used.",
                true,
            )),
            preview_tool_entry(tool(
                "read-1",
                "Read",
                "read",
                r#"{"path":"/work/workbench/src/export/markdown.rs"}"#,
            )),
            preview_tool_entry(tool("grep-1", "Search", "search", r#"{"pattern":"join(\"\\n"}"#)),
            preview_tool_entry(ToolCall {
                raw_output: Some("Updated 1 file.".into()),
                content: vec![ToolContent::Diff(ToolDiff {
                    path: "/work/workbench/src/export/markdown.rs".into(),
                    old_text: Some(
                        "fn render(blocks: &[Block]) -> String {\n    let body = blocks.join(\"\\n\\n\");\n    body\n}\n"
                            .into(),
                    ),
                    new_text: "fn render(blocks: &[Block], ending: LineEnding) -> String {\n    let separator = ending.repeat(2);\n    let body = blocks.join(&separator);\n    body + ending.as_str()\n}\n"
                        .into(),
                })],
                ..tool(
                    "edit-1",
                    "Edit",
                    "edit",
                    r#"{"path":"/work/workbench/src/export/markdown.rs"}"#,
                )
            }),
            preview_tool_entry(ToolCall {
                raw_output: Some(
                    "Created At: 2026-11-20T19:51:26-05:00\nCompleted At: 2026-11-20T19:51:31-05:00\nCommand exited with code 101.\n\nCommand output:\nrunning 12 tests\ntest export::crlf ... FAILED\n\nfailures:\n    export::crlf: expected \"a\\r\\n\\r\\nb\\r\\n\", got \"a\\r\\n\\r\\nb\\n\"\n".into(),
                ),
                ..tool(
                    "test-1",
                    "Run tests",
                    "execute",
                    r#"{"command":"cargo test -p workbench export::"}"#,
                )
            }),
            preview_tool_entry(tool(
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
                usage: None,
                stop_reason: Some("cancelled".into()),
                error: None,
                answer: None,
            }),
        ],
        None,
    );
    for key in ["1:0-activity", "acp-1-edit-1", "acp-1-test-1", "e10"] {
        toggle(m, key);
    }
}

pub(super) fn seed_subagents(m: &mut Model) {
    seed_workspace(m);
    let cache = m.cache_mut();
    cache.set_threads(list());
    cache.tasks = [
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
    let open = cache.open_thread_mut(THREAD);
    open.transcript = preview_thread([
        prompt("Compare the export against the old CLI, and review line endings."),
        response("I’ll delegate the export comparison and the line-ending review."),
        preview_tool_entry(ToolCall {
            viz: Some(VizLink {
                tool: "delegate_task".into(),
                child: Some(ThreadId(20)),
                environment: None,
            }),
            ..preview_tool("spawn-1", "Compare exports", "other", ToolCallStatus::InProgress)
        }),
        preview_tool_entry(ToolCall {
            viz: Some(VizLink {
                tool: "delegate_task".into(),
                child: Some(ThreadId(21)),
                environment: None,
            }),
            raw_output: Some("CRLF survives the join; only the final newline was missing.\nNo other changes needed.".into()),
            ..preview_tool("spawn-2", "Review line endings", "other", ToolCallStatus::Completed)
        }),
        preview_tool_entry(ToolCall {
            viz: Some(VizLink {
                tool: "thread_send".into(),
                child: Some(ThreadId(20)),
                environment: None,
            }),
            ..preview_tool("message-1", "Compare exports", "other", ToolCallStatus::Completed)
        }),
    ], None);
    if let Some(state) = &mut open.state {
        state.status = ThreadStatus::Working;
    }
}

pub(super) fn seed_tasks_expanded(m: &mut Model) {
    seed_subagents(m);
    let cache = m.cache_mut();
    cache.tasks.push(Task {
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
    cache.tasks.push(Task {
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
    toggle(m, composer::TASKS_DOCK_KEY);
}

pub(super) fn seed_agents(m: &mut Model) {
    seed_subagents(m);
    toggle(m, "acp-1-spawn-1-activity");
}

pub(super) fn seed_subagent(m: &mut Model) {
    seed_workspace(m);
    m.cache_mut().set_threads(list());
    select_preview(m, ThreadId(20));
    let mut state = thread_state();
    state.status = ThreadStatus::Working;
    state.usage = Some(ContextUsage {
        used: 34_817,
        size: 1_000_000,
    });
    state.live = Some(LiveTurn {
        turn: "1".into(),
        entries: vec![
            preview_text(
                "1:1",
                "Run both exporters against the same inputs and compare their output.",
                true,
            ),
            preview_entry(
                "1:2",
                Body::Tool(Box::new(ToolCall {
                    started_at: Some(now_ms() - 11_000),
                    raw_input: Some(r#"{"command":"diff -u old.md new.md","timeout":30}"#.into()),
                    ..preview_tool(
                        "compare",
                        "Compare sample exports",
                        "execute",
                        ToolCallStatus::InProgress,
                    )
                })),
            ),
        ],
    });
    let mut transcript = preview_thread(
        [
            prompt("Compare the markdown export with the old CLI’s output for the sample notes."),
            response("I’ll compare the output for `samples/`."),
        ],
        None,
    );
    transcript.sync_acp(state.live.as_ref());
    let open = m.cache_mut().open_thread_mut(ThreadId(20));
    open.loading = false;
    open.transcript = transcript;
    open.state = Some(state);
}

pub(super) fn seed_checkouts(m: &mut Model) {
    m.cache_mut().checkouts.insert(
        "/work/workbench".into(),
        Checkouts {
            branches: ["main", "release", "origin/main", "origin/release"]
                .map(|name| GitBranch {
                    name: name.into(),
                    remote: name.starts_with("origin/"),
                    upstream: None,
                    checked_out_elsewhere: false,
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

pub(super) fn list() -> Vec<ThreadSummary> {
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
        pull_request: None,
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
        forked_from: None,
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

pub(super) fn thread_state() -> ThreadState {
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
            source: viz_client::protocol::CommandSource::Command,
        }],
        queue: Vec::new(),
        attachments: AttachmentCapabilities { images: true },
        live: None,
        usage: None,
        access: viz_client::protocol::Access {
            current: viz_client::protocol::AccessMode {
                kind: viz_client::protocol::AccessKind::Full,
                label: "Full access".into(),
                native: None,
            },
            source: viz_client::protocol::AccessSource::Default,
            modes: Vec::new(),
        },
        requests: Vec::new(),
    }
}

pub(super) fn transcript(attachments: &[&str]) -> Transcript {
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
                context: Vec::new(),
                origin: None,
            },
            response(
                "The export drops the final newline when it joins blocks. I’ll fix that at the boundary.",
            ),
            preview_plan(
                [
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
            ),
            preview_tool_entry(ToolCall {
                raw_output: Some("Updated 1 file.".into()),
                content: vec![ToolContent::Diff(ToolDiff {
                    path: "src/export.rs".into(),
                    old_text: Some("let body = blocks.join(\"\n\n\");\nOk(body)\n".into()),
                    new_text: "let body = blocks.join(\"\n\n\");\nOk(format!(\"{body}\n\"))\n"
                        .into(),
                })],
                ..preview_tool(
                    "write-1",
                    "Update markdown export",
                    "edit",
                    ToolCallStatus::Completed,
                )
            }),
            EntryData::Run(RunEnd {
                usage: None,
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

pub(super) fn options() -> Vec<ConfigOption> {
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

pub(super) fn option(
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

fn preview_thread(
    entries: impl IntoIterator<Item = EntryData>,
    before: Option<EntryId>,
) -> Transcript {
    let entries = (1_u64..)
        .zip(entries)
        .map(|(id, data)| Entry {
            id: EntryId(id),
            data,
        })
        .collect::<Vec<_>>();
    Transcript::from_entries(&entries, before)
}

fn prompt(text: &str) -> EntryData {
    EntryData::Prompt {
        text: text.into(),
        attachments: Vec::new(),
        context: Vec::new(),
        origin: None,
    }
}

fn response(text: &str) -> EntryData {
    EntryData::Transcript(preview_text("1:0", text, false))
}

fn preview_text(key: &str, text: &str, thought: bool) -> TranscriptEntry {
    let segment = Segment {
        text: text.into(),
        message: key.into(),
        started_at: None,
        ended_at: None,
    };
    preview_entry(
        key,
        if thought {
            Body::Thought(segment)
        } else {
            Body::Text(segment)
        },
    )
}

fn preview_entry(key: &str, body: Body) -> TranscriptEntry {
    TranscriptEntry {
        key: key.into(),
        order: 0,
        body,
        more: false,
    }
}

fn preview_tool_entry(tool: ToolCall) -> EntryData {
    let key = tool.id.clone();
    EntryData::Transcript(preview_entry(&key, Body::Tool(Box::new(tool))))
}

fn preview_plan(entries: Vec<PlanEntry>) -> EntryData {
    EntryData::Transcript(preview_entry(
        "plan",
        Body::Plan(Plan {
            id: None,
            at: None,
            content: PlanContent::Items { entries },
        }),
    ))
}

fn preview_tool(id: &str, title: &str, kind: &str, status: ToolCallStatus) -> ToolCall {
    let mut tool = ToolCall::new(id.into());
    tool.title = title.into();
    tool.kind = Some(kind.into());
    tool.status = status;
    tool
}
