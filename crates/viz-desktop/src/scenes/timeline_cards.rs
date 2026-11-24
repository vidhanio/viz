//! Headless preview scenes for rich timeline cards, dividers,
//! subagent groups, proposed plan cards, and error banners.

use std::path::PathBuf;

use viz_client::protocol::{
    Body, Compaction, CompactionStatus, EntryData, ForkSource, Plan, PlanContent, PlanEntry,
    PlanPriority, PlanStatus, Task, TaskStatus, ThreadId, ToolCall, ToolCallStatus, VizLink,
};

use super::{
    data::{
        THREAD, list, preview_entry, preview_plan, preview_thread, preview_tool,
        preview_tool_entry, prompt,
    },
    fixtures::seed_workspace,
    timeline::{BASE_TS, run_end},
};
use crate::{app::Desktop, model::Model};

/// Rich timeline cards & dividers: Forked-from banner, overlapping-avatar
/// `SubagentGroup` (local + remote + failed), auto-collapsed Markdown
/// `ProposedPlanCard` + priority checklist, expanded `Compaction` summary,
/// and `UsageLimit` / `General` inline error banners.
pub(super) fn cards(s: &mut Desktop) {
    seed_cards(&mut s.model);
    s.sync_widgets_from_model();
}

fn seed_cards(m: &mut Model) {
    seed_workspace(m);
    m.timeline.clear_folds(Some(THREAD));
    m.disclosure.tool_clock = BASE_TS + 50_000;

    let mut threads = list();
    if let Some(active) = threads.iter_mut().find(|t| t.id == THREAD) {
        active.forked_from = Some(ForkSource {
            thread: ThreadId(10),
            entry: None,
        });
    }
    let cache = m.cache_mut();
    cache.set_threads(threads);
    cache.tasks = cards_tasks();
    cache.open_thread_mut(THREAD).transcript = preview_thread(cards_entries(), None);

    m.timeline
        .set_fold(Some(THREAD), "turn-e1-0-subagents".into(), true);
    m.timeline.set_fold(Some(THREAD), "e2-summary".into(), true);
}

fn cards_tasks() -> Vec<Task> {
    vec![
        Task {
            id: "20".into(),
            parent: THREAD,
            environment: "local".into(),
            thread: Some(ThreadId(20)),
            run: None,
            status: TaskStatus::Running,
            reachable: true,
            title: "Compare exports".into(),
            result: None,
            error: None,
        },
        Task {
            id: "21".into(),
            parent: THREAD,
            environment: "local".into(),
            thread: Some(ThreadId(21)),
            run: None,
            status: TaskStatus::Completed,
            reachable: true,
            title: "Review line endings".into(),
            result: Some("CRLF survives the join cleanly.".into()),
            error: None,
        },
        Task {
            id: "22".into(),
            parent: THREAD,
            environment: "build-box".into(),
            thread: Some(ThreadId(22)),
            run: None,
            status: TaskStatus::Completed,
            reachable: true,
            title: "Verify Linux release build".into(),
            result: Some("Release binary built in 28s.".into()),
            error: None,
        },
        Task {
            id: "23".into(),
            parent: THREAD,
            environment: "gpu-node".into(),
            thread: Some(ThreadId(23)),
            run: None,
            status: TaskStatus::Failed,
            reachable: true,
            title: "Run screenshot diff".into(),
            result: None,
            error: Some("Renderer exited with code 1".into()),
        },
    ]
}

fn cards_entries() -> Vec<EntryData> {
    let spawn = |id: &str, title: &str, child: u64, env: Option<&str>, status: ToolCallStatus| {
        preview_tool_entry(ToolCall {
            viz: Some(VizLink {
                tool: "delegate_task".into(),
                child: Some(ThreadId(child)),
                environment: env.map(str::to_owned),
            }),
            ..preview_tool(id, title, "other", status)
        })
    };

    let long_plan_md = "# Timeline Rebuild Rollout\n\n\
        ## Phase 1 — Projection & Turn Grouping\n\
        - Project committed and live `TranscriptEntry` records into `TurnView`.\n\
        - Preserve per-thread fold state across switches.\n\n\
        ## Phase 2 — Rich Rows & Inspectors\n\
        - Right-aligned user bubbles with `176pt` auto-collapse and hover actions.\n\
        - Borderless horizontal-rule Markdown tables and framed code blocks.\n\
        - Command output with ANSI SGR spans and lazy `thread.entry_chunk` loading.\n\n\
        ## Phase 3 — Cards and Dividers\n\
        - `ChangedFilesTree` with compacted folders and `+N -M` stats.\n\
        - Overlapping-avatar `SubagentGroup` with live status dots.\n\
        ## Phase 4 — Theme & Accessibility Verification\n\
        - Snap every timeline scene in `github-dark`, `github`, and `rose-pine-dawn`.\n\
        - Verify zero layout warnings in `--tree` JSON output.\n\
        - Run `treefmt --fail-on-change` and workspace clippy with `-D warnings`.";

    vec![
        prompt("Coordinate subagents, propose the rollout plan, and compact context."),
        EntryData::Transcript(preview_entry(
            "c1",
            Body::Compaction(Compaction {
                id: "cmp-1".into(),
                status: CompactionStatus::Completed,
                summary: "Retained export architecture, diff stats, and subagent contracts while trimming earlier verbose logs.".into(),
                error: None,
                tokens_before: Some(142_400),
                tokens_after: Some(28_600),
                started_at: BASE_TS,
                ended_at: BASE_TS + 2_100,
            }),
        )),
        spawn("s1", "Compare exports", 20, None, ToolCallStatus::InProgress),
        spawn("s2", "Review line endings", 21, None, ToolCallStatus::Completed),
        spawn("s3", "Verify Linux release build", 22, Some("build-box"), ToolCallStatus::Completed),
        spawn("s4", "Run screenshot diff", 23, Some("gpu-node"), ToolCallStatus::Completed),
        preview_plan(vec![
            PlanEntry {
                content: "Wire per-thread fold memory".into(),
                priority: PlanPriority::High,
                status: PlanStatus::Completed,
            },
            PlanEntry {
                content: "Verify contrast across all three base16 themes".into(),
                priority: PlanPriority::High,
                status: PlanStatus::InProgress,
            },
            PlanEntry {
                content: "Expose clean Track X pending-requests slot".into(),
                priority: PlanPriority::Medium,
                status: PlanStatus::Pending,
            },
        ]),
        EntryData::Transcript(preview_entry(
            "plan-md",
            Body::Plan(Plan {
                id: Some("rollout".into()),
                at: Some(BASE_TS + 5_000),
                content: PlanContent::Markdown {
                    text: long_plan_md.into(),
                    path: Some(PathBuf::from("/work/workbench/plan.md")),
                },
            }),
        )),
        run_end(
            None,
            None,
            Some("Usage limit reached for this 5-hour window. Resets at 19:00 UTC."),
        ),
        prompt("Retry with the fallback model."),
        run_end(
            None,
            None,
            Some("Harness connection dropped unexpectedly while reading tool output."),
        ),
    ]
}
