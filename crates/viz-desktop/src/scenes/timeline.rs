//! Headless preview scenes verifying rich timeline turns, expanded work logs,
//! and live working state across dark and light base16 themes.

use serde_json::Value;
use viz_client::protocol::{
    Attention, Body, EntryData, Exit, LiveTurn, PromptAttachment, PromptContext, RunEnd, Segment,
    SettledBy, Shelf, Standing, ThreadId, ThreadStatus, ToolCallStatus, ToolContent, ToolDiff,
    TranscriptEntry, TurnTokens, TurnUsage, UsageCost,
};

use super::{
    data::{
        THREAD, now_ms, preview_text, preview_thread, preview_tool, preview_tool_entry, prompt,
    },
    fixtures::seed_workspace,
};
use crate::{app::Desktop, model::Model};

/// Deterministic base Unix timestamp (ms) so clocks and durations stay stable.
pub(super) const BASE_TS: i64 = 1_763_908_320_000;

/// Populated multi-turn timeline: stacked user prompts, context chips,
/// attachment cards, auto-collapsed long prompt, collapsed Completed Turn Fold,
/// Markdown table + framed code block, `ChangedFilesTree`, and turn footer.
pub(super) fn populated(s: &mut Desktop) {
    seed_populated(&mut s.model);
    s.sync_widgets_from_model();
}

/// Expanded `WorkLog` showing summary header, reasoning inspector, inline diff
/// card, ANSI-coloured command output with exit footer & lazy chunk bar,
/// structured search matches, clickable no-output file read, and MCP JSON I/O.
pub(super) fn expanded(s: &mut Desktop) {
    seed_expanded(&mut s.model);
    s.sync_widgets_from_model();
}

/// Centered file preview modal showing syntax-highlighted Rust source with a
/// highlighted `#L8-L13` range badge and gutter accent.
pub(super) fn file_preview(s: &mut Desktop) {
    seed_populated(&mut s.model);
    let sample = r"//! Prompt assembly helpers for ACP harnesses.

use viz_protocol::{Attachment, PromptContext};

/// Builds the user prompt text and typed ACP content blocks.
pub fn build_prompt(
    text: &str,
    attachments: &[Attachment],
    context: &[PromptContext],
) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();
    let body = append_attachment_refs(text, attachments);
    blocks.push(ContentBlock::text(body));
    for attachment in attachments {
        push_media_block(&mut blocks, attachment);
    }
    blocks
}
";
    s.model.timeline.file_preview = Some(crate::model::FilePreview {
        request: 1,
        path: "/work/workbench/crates/viz-acp/src/prompt.rs".into(),
        lines: Some((6, 13)),
        open_url: Some("file:///work/workbench/crates/viz-acp/src/prompt.rs#L6-L13".into()),
        body: crate::model::FilePreviewBody::Text {
            text: sample.into(),
            start_line: 1,
            truncated: false,
            size: sample.len() as u64,
        },
    });
    s.sync_widgets_from_model();
}

/// Showcases rich Markdown rendering inside sent user bubbles (headings, bold,
/// code, inline `@file` pill, `[spec.pdf][file-1]` attachment pill, inline
/// embedded `![Clipboard.png][img-1]` image), compact fit-content `"continue"`
/// bubble width, and clickable `file://` pills inside assistant prose.
pub(super) fn user_markdown(s: &mut Desktop) {
    seed_workspace(&mut s.model);
    let env = s.model.active_env_key().to_owned();
    s.model.timeline.clear_folds(&env, Some(THREAD));
    s.model.disclosure.tool_clock = BASE_TS + 30_000;

    let sample_svg =
        b"<svg xmlns='http://www.w3.org/2000/svg' width='320' height='120' viewBox='0 0 320 120'>\
        <rect width='320' height='120' rx='12' fill='#1c2128'/>\
        <rect x='14' y='16' width='292' height='26' rx='6' fill='#262c36'/>\
        <circle cx='30' cy='29' r='5' fill='#f85149'/>\
        <circle cx='46' cy='29' r='5' fill='#d29922'/>\
        <circle cx='62' cy='29' r='5' fill='#3fb950'/>\
        <rect x='14' y='54' width='180' height='14' rx='4' fill='#58a6ff' fill-opacity='0.75'/>\
        <rect x='14' y='78' width='250' height='10' rx='3' fill='#8b949e' fill-opacity='0.5'/>\
        <rect x='14' y='96' width='140' height='10' rx='3' fill='#8b949e' fill-opacity='0.35'/>\
        </svg>";

    let entries = vec![
        EntryData::Prompt {
            text: "continue".into(),
            attachments: Vec::new(),
            context: Vec::new(),
            origin: None,
        },
        timed_text(
            "1:1",
            "Looked at [prompt.rs#L6-L13](file:///work/workbench/crates/viz-acp/src/prompt.rs#L6-L13) and [user_row.rs](file:///work/workbench/crates/viz-desktop/src/timeline/user_row.rs) — everything aligns cleanly.",
            false,
            BASE_TS,
            BASE_TS + 1_200,
        ),
        run_end(Some("end_turn"), Some(usage(1_800, 220, 0.02)), None),
        EntryData::Prompt {
            text: "### Check this layout bug\n\n\
                Please compare @markdown.rs against [export-spec.pdf][file-1]:\n\
                - **Bold**, *italic*, ~~old width~~, and `inline_code`\n\
                - Inline screenshot right here:\n\n\
                ![Clipboard.png][img-1]"
                .into(),
            attachments: vec![
                PromptAttachment {
                    id: "demo-img-1".into(),
                    name: "Clipboard.png".into(),
                    mime: "image/svg+xml".into(),
                },
                PromptAttachment {
                    id: "demo-file-1".into(),
                    name: "export-spec.pdf".into(),
                    mime: "application/pdf".into(),
                },
            ],
            context: vec![PromptContext::File {
                path: "crates/export/src/markdown.rs".into(),
            }],
            origin: None,
        },
        timed_text(
            "2:1",
            "Got it — I checked [markdown.rs](file:///work/workbench/crates/export/src/markdown.rs#L12-L24) and matched the screenshot.",
            false,
            BASE_TS + 2_000,
            BASE_TS + 3_500,
        ),
        run_end(Some("end_turn"), Some(usage(2_400, 310, 0.03)), None),
    ];

    s.model.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(entries, None);
    s.decode_thread_attachment_image(
        s.model.active_env_key().to_owned(),
        THREAD,
        "demo-img-1".into(),
        "image/svg+xml",
        sample_svg.to_vec(),
    );
    s.sync_widgets_from_model();
}

/// Active live turn with ticking Working header, collapsed earlier steps,
/// streaming thinking preview, running ANSI command with timeout ring, and
/// streaming assistant prose.
pub(super) fn working(s: &mut Desktop) {
    seed_working(&mut s.model);
    s.sync_widgets_from_model();
}

/// Settled thread with a child subagent awaiting approval so both
/// bottom-of-chat status line families (`ThreadNow` + `ThreadStatusLine`)
/// render together.
pub(super) fn status_line(s: &mut Desktop) {
    seed_populated(&mut s.model);
    let now = BASE_TS + 2 * 3_600_000;
    s.model.disclosure.tool_clock = now;
    if let Some(thread) = s.model.cache_mut().summary_mut(THREAD) {
        thread.shelf = Shelf::Settled;
        thread.lifecycle.standing = Standing::Settled {
            at: BASE_TS,
            by: SettledBy::User,
        };
    }
    if let Some(mut child) = s.model.cache().summary(THREAD).cloned() {
        child.id = ThreadId(99);
        child.title = "Verify release build".into();
        child.parent = Some(THREAD);
        child.shelf = Shelf::Working;
        child.lifecycle.standing = Standing::Open;
        child.attention = Some(Attention::Approval);
        let mut threads: Vec<_> = s.model.cache().threads().cloned().collect();
        threads.push(child);
        s.model.cache_mut().set_threads(threads);
    }
    s.sync_widgets_from_model();
}

fn seed_populated(m: &mut Model) {
    seed_workspace(m);
    let env = m.active_env_key().to_owned();
    m.timeline.clear_folds(&env, Some(THREAD));
    m.disclosure.tool_clock = BASE_TS + 60_000;
    if let Some(thread) = m.cache_mut().summary_mut(THREAD) {
        thread.model = Some("Claude Opus 4.7".into());
    }
    let mut entries = populated_turn_one();
    entries.extend(populated_turn_two());
    m.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(entries, None);
}

fn populated_turn_one() -> Vec<EntryData> {
    let long_prompt = "Please also verify every edge case in the export pipeline:\n\
        1. Trailing CRLF preservation across Windows-authored documents.\n\
        2. Nested fenced code blocks containing triple backticks inside four-backtick fences.\n\
        3. Borderless GFM tables with alignment markers (`:---`, `:---:`, `---:`).\n\
        4. Empty frontmatter blocks and UTF-8 BOM handling at byte offset zero.\n\
        5. Multi-byte grapheme clusters in heading anchors and footnote references.\n\
        6. Inline code spans containing escaped pipes inside Markdown table cells.\n\
        7. Long unbreakable URLs inside blockquote callouts.\n\
        8. Deterministic output across macOS, Linux, and headless snapshot runs.\n\
        9. Preserve existing file permissions when writing atomic temp files.";

    vec![
        EntryData::Prompt {
            text: long_prompt.into(),
            attachments: Vec::new(),
            context: Vec::new(),
            origin: None,
        },
        timed_tool(
            "cmd-check",
            "Run verification",
            "execute",
            Some(r#"{"command":"cargo clippy --workspace -- -D warnings"}"#),
            Some("Finished dev profile in 3.2s"),
            Vec::new(),
            BASE_TS,
            BASE_TS + 5_200,
        ),
        timed_text(
            "1:1",
            "All nine edge cases are covered and the workspace builds cleanly with zero warnings.",
            false,
            BASE_TS + 5_200,
            BASE_TS + 6_000,
        ),
        run_end(Some("end_turn"), Some(usage(4_210, 620, 0.04)), None),
    ]
}

fn populated_turn_two() -> Vec<EntryData> {
    let table_and_code = "Updated the export pipeline and verified all line-ending modes.\n\n\
        | Format | Trailing newline | Line endings | Status |\n\
        | :--- | :--- | :--- | :--- |\n\
        | CommonMark | Preserved | LF / CRLF | Passing |\n\
        | GFM Table | Preserved | LF | Passing |\n\
        | Frontmatter | Normalized | Source | Passing |\n\n\
        ```rust\n\
        pub fn join_blocks(blocks: &[Block], ending: LineEnding) -> String {\n\
            let sep = ending.repeat(2);\n\
            format!(\"{}{}\", blocks.join(&sep), ending.as_str())\n\
        }\n\
        ```\n\n\
        The helper stays allocation-minimal and preserves `ending` verbatim.";

    vec![
        EntryData::Prompt {
            text: "Audit `markdown.rs` and add nested folder support.".into(),
            attachments: vec![PromptAttachment {
                id: "att-1".into(),
                name: "export-spec.pdf".into(),
                mime: "application/pdf".into(),
            }],
            context: vec![
                PromptContext::File {
                    path: "crates/export/src/markdown.rs".into(),
                },
                PromptContext::Link {
                    uri: "https://example.invalid/issues/42".into(),
                    name: "#42".into(),
                    title: Some("Preserve final newline".into()),
                },
            ],
            origin: None,
        },
        prompt("Keep the public `join_blocks` signature unchanged if possible."),
        timed_tool(
            "read-1",
            "Read",
            "read",
            Some(r#"{"path":"/work/workbench/crates/export/src/markdown.rs"}"#),
            None,
            Vec::new(),
            BASE_TS + 10_000,
            BASE_TS + 11_200,
        ),
        timed_tool(
            "edit-1",
            "Edit",
            "edit",
            Some(r#"{"path":"/work/workbench/crates/export/src/markdown.rs"}"#),
            Some("Updated 1 file."),
            vec![ToolContent::Diff(ToolDiff {
                path: "/work/workbench/crates/export/src/markdown.rs".into(),
                old_text: Some("let body = blocks.join(\"\\n\\n\");\nbody\n".into()),
                new_text: "let sep = ending.repeat(2);\nlet body = blocks.join(&sep);\nformat!(\"{body}{}\", ending.as_str())\n".into(),
            })],
            BASE_TS + 11_500,
            BASE_TS + 14_100,
        ),
        timed_tool(
            "edit-2",
            "Edit",
            "edit",
            Some(r#"{"path":"/work/workbench/crates/export/src/line_ending.rs"}"#),
            Some("Created helper."),
            vec![ToolContent::Diff(ToolDiff {
                path: "/work/workbench/crates/export/src/line_ending.rs".into(),
                old_text: None,
                new_text: "pub enum LineEnding { Lf, Crlf }\nimpl LineEnding {\n    pub fn as_str(&self) -> &'static str { match self { Self::Lf => \"\\n\", Self::Crlf => \"\\r\\n\" } }\n}\n".into(),
            })],
            BASE_TS + 14_200,
            BASE_TS + 16_300,
        ),
        timed_tool(
            "edit-3",
            "Edit",
            "edit",
            Some(r#"{"path":"/work/workbench/Cargo.toml"}"#),
            Some("Updated metadata."),
            vec![ToolContent::Diff(ToolDiff {
                path: "/work/workbench/Cargo.toml".into(),
                old_text: Some("version = \"0.1.0\"\n".into()),
                new_text: "version = \"0.2.0\"\n".into(),
            })],
            BASE_TS + 16_400,
            BASE_TS + 18_100,
        ),
        timed_text(
            "2:1",
            table_and_code,
            false,
            BASE_TS + 18_100,
            BASE_TS + 19_400,
        ),
        run_end(Some("end_turn"), Some(usage(12_400, 1_850, 0.14)), None),
    ]
}

#[expect(
    clippy::too_many_lines,
    reason = "seeds every tool inspector variant in one deterministic timeline fixture"
)]
fn seed_expanded(m: &mut Model) {
    seed_workspace(m);
    let env = m.active_env_key().to_owned();
    m.timeline.clear_folds(&env, Some(THREAD));
    m.disclosure.tool_clock = BASE_TS + 30_000;

    let ansi_out = "\x1b[1mrunning 4 checks\x1b[0m\n\
        \x1b[32m  ok\x1b[0m export::commonmark_roundtrip\n\
        \x1b[32m  ok\x1b[0m export::table_alignment\n\
        \x1b[33mwarn\x1b[0m export::bom_strip (deprecated flag)\n\
        \x1b[31mFAIL\x1b[0m export::crlf_trailing_newline\n\
        expected \"a\\r\\n\\r\\nb\\r\\n\", got \"a\\r\\n\\r\\nb\\n\"";

    let mut cmd_tool = preview_tool(
        "cmd-1",
        "Run export checks",
        "execute",
        ToolCallStatus::Completed,
    );
    cmd_tool.raw_input = Some(json_value(
        r#"{"command":"cargo check -p workbench --all-targets"}"#,
    ));
    cmd_tool.raw_output = Some(Value::String(ansi_out.into()));
    cmd_tool.exit = Some(Exit {
        code: Some(101),
        signal: None,
    });
    cmd_tool.started_at = Some(BASE_TS + 3_000);
    cmd_tool.ended_at = Some(BASE_TS + 8_400);
    cmd_tool.more = true;

    let mut search_tool = preview_tool("search-1", "Search", "search", ToolCallStatus::Completed);
    search_tool.raw_input = Some(json_value(r#"{"query":"blocks.join"}"#));
    search_tool.raw_output = Some(Value::String(
        "/work/workbench/src/export/markdown.rs:42: let body = blocks.join(\"\\n\\n\");\n\
         /work/workbench/src/export/html.rs:19: let out = blocks.join(\"\\n\");"
            .into(),
    ));
    search_tool.started_at = Some(BASE_TS + 1_000);
    search_tool.ended_at = Some(BASE_TS + 1_400);

    let mut edit_tool = preview_tool("edit-1", "Edit", "edit", ToolCallStatus::Completed);
    edit_tool.raw_input = Some(json_value(
        r#"{"path":"/work/workbench/src/export/markdown.rs"}"#,
    ));
    edit_tool.content = vec![ToolContent::Diff(ToolDiff {
        path: "/work/workbench/src/export/markdown.rs".into(),
        old_text: Some("fn render(blocks: &[Block]) -> String {\n    blocks.join(\"\\n\\n\")\n}\n".into()),
        new_text: "fn render(blocks: &[Block], eol: &str) -> String {\n    let sep = format!(\"{eol}{eol}\");\n    format!(\"{}{eol}\", blocks.join(&sep))\n}\n".into(),
    })];
    edit_tool.started_at = Some(BASE_TS + 1_800);
    edit_tool.ended_at = Some(BASE_TS + 2_900);

    let mut mcp_tool = preview_tool(
        "mcp-1",
        "mcp__docs__lookup",
        "other",
        ToolCallStatus::Completed,
    );
    mcp_tool.name = Some("mcp__docs__lookup".into());
    mcp_tool.raw_input = Some(json_value(
        r#"{"spec":"CommonMark 0.31","section":"line-endings"}"#,
    ));
    mcp_tool.raw_output = Some(json_value(
        r#"{"matched":true,"rule":"Preserve source EOL sequences"}"#,
    ));
    mcp_tool.started_at = Some(BASE_TS + 8_500);
    mcp_tool.ended_at = Some(BASE_TS + 9_200);

    let mut read_tool = preview_tool("read-1", "Read File", "read", ToolCallStatus::Completed);
    read_tool.raw_input = Some(json_value(
        r#"{"path":"/work/workbench/crates/viz-acp/src/prompt.rs","StartLine":6,"EndLine":13}"#,
    ));
    read_tool.started_at = Some(BASE_TS + 950);
    read_tool.ended_at = Some(BASE_TS + 1_000);

    let entries = [
        prompt("Fix CRLF preservation and inspect the failing check."),
        timed_text(
            "1:0",
            "Check where blocks are joined in `markdown.rs`. The literal `\\n\\n` hardcodes LF regardless of the source document's line ending.",
            true,
            BASE_TS,
            BASE_TS + 900,
        ),
        preview_tool_entry(read_tool),
        preview_tool_entry(search_tool),
        preview_tool_entry(edit_tool),
        preview_tool_entry(cmd_tool),
        preview_tool_entry(mcp_tool),
        timed_text(
            "1:5",
            "Found the trailing-newline mismatch in `export::crlf_trailing_newline`; applying the final separator fix next.",
            false,
            BASE_TS + 9_400,
            BASE_TS + 10_100,
        ),
        run_end(Some("end_turn"), Some(usage(8_300, 1_120, 0.09)), None),
    ];

    m.cache_mut().open_thread_mut(THREAD).transcript = preview_thread(entries, None);
    for key in [
        "turn-e1-0-fold",
        "1:0",
        "acp-1-search-1",
        "acp-1-edit-1",
        "acp-1-cmd-1",
        "acp-1-mcp-1",
    ] {
        m.timeline.set_fold(&env, Some(THREAD), key.into(), true);
    }
}

fn seed_working(m: &mut Model) {
    seed_workspace(m);
    let env = m.active_env_key().to_owned();
    m.timeline.clear_folds(&env, Some(THREAD));
    let now = now_ms();
    let start = now - 14_000;
    m.disclosure.tool_clock = now;

    let live = working_live_turn(start, now);
    let mut transcript = preview_thread(working_committed_entries(start), None);
    transcript.sync_acp(Some(&live));

    let open = m.cache_mut().open_thread_mut(THREAD);
    open.transcript = transcript;
    if let Some(state) = &mut open.state {
        state.status = ThreadStatus::Working;
        state.live = Some(live);
    }
    m.timeline
        .set_fold(&env, Some(THREAD), "acp-1-run-live".into(), true);
}

fn working_live_turn(start: i64, now: i64) -> LiveTurn {
    let mut running_cmd = preview_tool(
        "run-live",
        "Run build",
        "execute",
        ToolCallStatus::InProgress,
    );
    running_cmd.raw_input = Some(json_value(
        r#"{"command":"cargo clippy --workspace --all-targets","timeout":60}"#,
    ));
    running_cmd.raw_output = Some(Value::String(
        "\x1b[32m   Compiling\x1b[0m viz-protocol v0.1.0\n\
         \x1b[32m   Compiling\x1b[0m viz-client v0.1.0\n\
         \x1b[36m    Checking\x1b[0m viz-desktop v0.1.0"
            .into(),
    ));
    running_cmd.started_at = Some(start + 2_000);

    LiveTurn {
        turn: "1".into(),
        entries: vec![
            TranscriptEntry {
                key: "1:5".into(),
                order: 5,
                body: Body::Thought(Segment {
                    text: "Verifying that the timeline projection keeps live active work rows visible while earlier read steps fold cleanly under the working header.".into(),
                    message: "1:5".into(),
                    started_at: Some(start + 1_500),
                    ended_at: Some(now),
                }),
                more: false,
            },
            TranscriptEntry {
                key: "1:6".into(),
                order: 6,
                body: Body::Tool(Box::new(running_cmd)),
                more: false,
            },
            TranscriptEntry {
                key: "1:7".into(),
                order: 7,
                body: Body::Text(Segment {
                    text: "Compiling the workspace to verify every timeline module passes strict clippy lints".into(),
                    message: "1:7".into(),
                    started_at: Some(start + 12_000),
                    ended_at: Some(now),
                }),
                more: false,
            },
        ],
    }
}

fn working_committed_entries(start: i64) -> Vec<EntryData> {
    vec![
        prompt("Rebuild the timeline view and verify strict clippy passes."),
        timed_tool(
            "r1",
            "Read",
            "read",
            Some(r#"{"path":"/work/workbench/src/main.rs"}"#),
            None,
            Vec::new(),
            start,
            start + 300,
        ),
        timed_tool(
            "r2",
            "Read",
            "read",
            Some(r#"{"path":"/work/workbench/src/shell.rs"}"#),
            None,
            Vec::new(),
            start + 300,
            start + 600,
        ),
        timed_tool(
            "r3",
            "Read",
            "read",
            Some(r#"{"path":"/work/workbench/src/model/mod.rs"}"#),
            None,
            Vec::new(),
            start + 600,
            start + 900,
        ),
        timed_tool(
            "r4",
            "Read",
            "read",
            Some(r#"{"path":"/work/workbench/src/ui/mod.rs"}"#),
            None,
            Vec::new(),
            start + 900,
            start + 1_200,
        ),
    ]
}

#[expect(
    clippy::too_many_arguments,
    reason = "compact deterministic fixture constructor for preview tool calls"
)]
fn timed_tool(
    id: &str,
    title: &str,
    kind: &str,
    input: Option<&str>,
    output: Option<&str>,
    content: Vec<ToolContent>,
    started_at: i64,
    ended_at: i64,
) -> EntryData {
    let mut tool = preview_tool(id, title, kind, ToolCallStatus::Completed);
    tool.raw_input = input.map(json_value);
    tool.raw_output = output.map(json_value);
    tool.content = content;
    tool.started_at = Some(started_at);
    tool.ended_at = Some(ended_at);
    preview_tool_entry(tool)
}

pub(super) fn timed_text(
    key: &str,
    body: &str,
    thought: bool,
    started_at: i64,
    ended_at: i64,
) -> EntryData {
    let mut entry = preview_text(key, body, thought);
    match &mut entry.body {
        Body::Text(seg) | Body::Thought(seg) => {
            seg.started_at = Some(started_at);
            seg.ended_at = Some(ended_at);
        }
        _ => {}
    }
    EntryData::Transcript(entry)
}

pub(super) fn usage(input_tokens: u64, output_tokens: u64, usd: f64) -> TurnUsage {
    TurnUsage {
        context: None,
        cost: serde_json::Number::from_f64(usd).map(|amount| UsageCost {
            currency: "USD".into(),
            amount,
        }),
        tokens: Some(TurnTokens {
            total_tokens: input_tokens + output_tokens,
            input_tokens,
            output_tokens,
            thought_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
        }),
    }
}

pub(super) fn run_end(
    stop_reason: Option<&str>,
    usage: Option<TurnUsage>,
    error: Option<&str>,
) -> EntryData {
    EntryData::Run(RunEnd {
        usage,
        stop_reason: stop_reason.map(str::to_owned),
        error: error.map(str::to_owned),
        answer: None,
    })
}

fn json_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}
