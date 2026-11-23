//! Headless preview scenes verifying Track L's rich timeline cards, dividers,
//! subagent groups, proposed plan cards, error banners, and minimap overlays.

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
    timeline::{BASE_TS, run_end, timed_text, usage},
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

/// Multi-turn thread on a wide viewport with left-gutter `TimelineMinimap`,
/// flat `320pt` hover preview popover, and floating `ScrollToEndButton` pill.
pub(super) fn minimap(s: &mut Desktop) {
    seed_minimap(&mut s.model);
    s.sync_widgets_from_model();
}

fn seed_cards(m: &mut Model) {
    seed_workspace(m);
    m.disclosure.expanded.clear();
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
        ## Phase 3 — Cards, Dividers & Minimap\n\
        - `ChangedFilesTree` with compacted folders and `+N -M` stats.\n\
        - Overlapping-avatar `SubagentGroup` with live status dots.\n\
        - Left-gutter `TimelineMinimap` with flat `320pt` hover preview.\n\n\
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

fn seed_minimap(m: &mut Model) {
    seed_workspace(m);
    m.disclosure.expanded.clear();
    m.panels.window_width = 1440.0;
    m.panels.sidebar.open = true;
    m.panels.sidebar.width = 240.0;

    let entries = [
        prompt("1. Summarize the current SQLite schema for threads and entries."),
        timed_text(
            "1:0",
            "Threads store lifecycle, shelf, harness, and fork lineage; entries hold prompt, transcript, and run-end records.",
            false,
            BASE_TS,
            BASE_TS + 2_000,
        ),
        run_end(Some("end_turn"), Some(usage(2_100, 320, 0.02)), None),
        prompt("2. How should `TimelineMinimap` jump to earlier turns cleanly?"),
        timed_text(
            "2:0",
            "Click each static-width turn tick to call `Tx::scroll_key_center_at(turn.key, 180.0)`, and highlight visible turns via `El::on_visible`.",
            false,
            BASE_TS + 60_000,
            BASE_TS + 64_000,
        ),
        run_end(Some("end_turn"), Some(usage(3_400, 480, 0.03)), None),
        prompt("3. Add the floating `ScrollToEndButton` pill when scrolled up."),
        timed_text(
            "3:0",
            "Mounted a flat `t.surface_raised` pill with a `1px` `t.border_overlay` hairline and an accent unseen-activity dot.",
            false,
            BASE_TS + 120_000,
            BASE_TS + 125_000,
        ),
        run_end(Some("end_turn"), Some(usage(4_050, 510, 0.04)), None),
        prompt("4. Confirm zero drop shadows remain on floating surfaces."),
        timed_text(
            "4:0",
            "Verified: every popover, menu, card, and floating pill uses strictly flat base16 fills and crisp `1px` hairlines.",
            false,
            BASE_TS + 180_000,
            BASE_TS + 183_000,
        ),
        run_end(Some("end_turn"), Some(usage(4_600, 410, 0.04)), None),
    ];

    m.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(entries, None);
    m.timeline.visible_turns.insert(1);
    m.timeline.visible_turns.insert(2);
    m.timeline.hovered_minimap = Some(1);
    m.timeline.far_from_bottom = true;
    m.timeline.unseen_activity = true;
}
