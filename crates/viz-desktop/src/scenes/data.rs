//! Pure protocol fixture constructors shared across desktop preview scenes.

use viz_client::protocol::{
    AttachmentCapabilities, Attention, Body, ConfigOption, DependentChoice, DependentOption, Entry,
    EntryData, EntryId, Lifecycle, OptionChoice, OptionKind, OptionValue, Plan, PlanContent,
    PlanEntry, PlanPriority, PlanStatus, PromptAttachment, RunEnd, Segment, Shelf, SlashCommand,
    ThreadId, ThreadState, ThreadStatus, ThreadSummary, ThreadWorktree, ToolCall, ToolCallStatus,
    ToolContent, ToolDiff, TranscriptEntry,
};

use crate::model::Transcript;

pub(super) const THREAD: ThreadId = ThreadId(11);
pub(super) const WORKTREE: &str = "/state/worktrees/workbench/viz-trim-export-whitespace";

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
    let mut prompt =
        viz_client::protocol::Prompt::new("Make the markdown export preserve a final newline.");
    for &name in attachments {
        prompt.append_attachment(PromptAttachment {
            id: name.to_owned(),
            name: name.to_owned(),
            mime: "image/png".into(),
        });
    }
    preview_thread(
        [
            EntryData::Prompt {
                prompt,
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
                    skip: 0,
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

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|now| i64::try_from(now.as_millis()).ok())
        .unwrap_or_default()
}

pub(super) fn preview_thread(
    entries: impl IntoIterator<Item = EntryData>,
    before: Option<EntryId>,
) -> Transcript {
    let entries = (1_u64..)
        .zip(entries)
        .map(|(id, data)| Entry {
            id: EntryId(id),
            full: None,
            data,
        })
        .collect::<Vec<_>>();
    Transcript::from_entries(&entries, before)
}

pub(super) fn prompt(text: &str) -> EntryData {
    EntryData::Prompt {
        prompt: viz_client::protocol::Prompt::new(text),
        origin: None,
    }
}

pub(super) fn response(text: &str) -> EntryData {
    EntryData::Transcript(preview_text("1:0", text, false))
}

pub(super) fn preview_text(key: &str, text: &str, thought: bool) -> TranscriptEntry {
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

pub(super) fn preview_entry(key: &str, body: Body) -> TranscriptEntry {
    TranscriptEntry {
        key: key.into(),
        order: 0,
        body,
    }
}

pub(super) fn preview_tool_entry(tool: ToolCall) -> EntryData {
    let key = tool.id.clone();
    EntryData::Transcript(preview_entry(&key, Body::Tool(Box::new(tool))))
}

pub(super) fn preview_plan(entries: Vec<PlanEntry>) -> EntryData {
    EntryData::Transcript(preview_entry(
        "plan",
        Body::Plan(Plan {
            id: None,
            at: None,
            content: PlanContent::Items { entries },
        }),
    ))
}

pub(super) fn preview_tool(id: &str, title: &str, kind: &str, status: ToolCallStatus) -> ToolCall {
    let mut tool = ToolCall::new(id.into());
    tool.title = title.into();
    tool.kind = Some(kind.into());
    tool.status = status;
    tool
}
