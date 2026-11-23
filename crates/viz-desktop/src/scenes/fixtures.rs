//! Pure [`Model`] fixture builders for timeline, workspace, and subagent
//! scenes.

use viz_client::protocol::{
    Body, ContextUsage, EntryData, Environment, EnvironmentStatus, GitBranch, LiveTurn, PlanEntry,
    PlanPriority, PlanStatus, Project, RunEnd, Task, TaskStatus, ThreadId, ThreadStatus, ToolCall,
    ToolCallStatus, ToolContent, ToolDiff, VizLink, Worktree,
};

use super::data::{
    THREAD, WORKTREE, list, now_ms, preview_entry, preview_plan, preview_text, preview_thread,
    preview_tool, preview_tool_entry, prompt, response, thread_state, transcript,
};
use crate::model::{Checkouts, Model, PaneSwitch, Status, Transcript};

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

pub(super) fn seed_thread_details_subagents(m: &mut Model) {
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
    if let Some(state) = &mut cache.open_thread_mut(THREAD).state {
        state.usage = Some(ContextUsage {
            used: 34_817,
            size: 200_000,
        });
    }
    m.panels.details_open = true;
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
