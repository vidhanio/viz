//! Maps ACP transcript and options to the server's wire types.

use viz_acp as acp;
pub(crate) use viz_acp::{
    Catalog, Desired, Mcp, Outcome, Output, Prompt, Selection, Session, SessionSnapshot,
    StopReason, Transcript, bridge, catalog, drain_processes, mcp, stop_reason_name,
};

use crate::wire;

pub(crate) fn live_turn(live: acp::LiveTurn) -> wire::LiveTurn {
    let mut text = None;
    let mut tools = Vec::new();
    for entry in live.entries {
        let (segment, kind) = match entry.body {
            acp::Body::Text(segment) => (segment, wire::LiveTextKind::Text),
            acp::Body::Thought(segment) => (segment, wire::LiveTextKind::Thought),
            acp::Body::Tool(record) => {
                tools.push(tool(&record));
                continue;
            }
            _ => continue,
        };
        text = Some(wire::LiveText {
            key: entry.key,
            text: segment.text,
            kind,
        });
    }
    wire::LiveTurn {
        turn: live.turn,
        text,
        tools,
    }
}

pub(crate) fn entry(entry: acp::Entry) -> Option<wire::NewEntry> {
    Some(match entry.body {
        acp::Body::Text(segment) => wire::NewEntry::Text {
            text: segment.text,
            key: entry.key,
        },
        acp::Body::Thought(segment) => wire::NewEntry::Thought {
            text: segment.text,
            key: entry.key,
        },
        acp::Body::Tool(record) => wire::NewEntry::Tool(tool(&record)),
        acp::Body::Media(_) => return None,
        acp::Body::Plan(plan) => {
            let acp::PlanContent::Items { entries } = plan.content else {
                return None;
            };
            wire::NewEntry::Plan {
                entries: entries
                    .into_iter()
                    .map(|entry| wire::PlanEntry {
                        content: entry.content,
                        priority: match entry.priority {
                            acp::PlanPriority::High => wire::PlanPriority::High,
                            acp::PlanPriority::Medium => wire::PlanPriority::Medium,
                            acp::PlanPriority::Low => wire::PlanPriority::Low,
                        },
                        status: match entry.status {
                            acp::PlanStatus::Pending => wire::PlanStatus::Pending,
                            acp::PlanStatus::InProgress => wire::PlanStatus::InProgress,
                            acp::PlanStatus::Completed => wire::PlanStatus::Completed,
                        },
                    })
                    .collect(),
            }
        }
    })
}

pub(crate) fn tool(record: &acp::ToolCall) -> wire::ToolRecord {
    let mut locations = Vec::new();
    for location in &record.locations {
        let path = location.path.to_string_lossy();
        if !locations
            .iter()
            .any(|previous: &String| previous == path.as_ref())
        {
            locations.push(path.into_owned());
        }
    }
    wire::ToolRecord {
        id: record.id.clone(),
        title: record.title.clone(),
        name: record.name.clone(),
        kind: record
            .kind
            .filter(|kind| *kind != acp::schema::ToolKind::Other)
            .and_then(|kind| serde_json::to_value(kind).ok()?.as_str().map(str::to_owned)),
        status: match record.status {
            acp::ToolStatus::Pending => wire::ToolCallStatus::Pending,
            acp::ToolStatus::InProgress => wire::ToolCallStatus::InProgress,
            acp::ToolStatus::Completed => wire::ToolCallStatus::Completed,
            acp::ToolStatus::Failed => wire::ToolCallStatus::Failed,
        },
        viz: record.viz.as_ref().map(|viz| viz.tool.clone()),
        child: record.viz.as_ref().and_then(|viz| viz.child),
        child_environment: record.viz.as_ref().and_then(|viz| viz.environment.clone()),
        started_at: record.started_at,
        input: record.input_text(),
        output: record.output_text(),
        locations,
        diffs: record
            .diffs()
            .map(|diff| wire::ToolDiff {
                path: diff.path.to_string_lossy().into_owned(),
                old_text: diff.old_text.clone(),
                new_text: diff.new_text.clone(),
            })
            .collect(),
    }
}

pub(crate) fn tool_input(record: wire::ToolRecord) -> acp::ToolCall {
    let mut call = acp::ToolCall::new(record.id);
    call.title = record.title;
    call.name = record.name;
    call.kind = record
        .kind
        .and_then(|kind| serde_json::from_value(kind.into()).ok());
    call.status = match record.status {
        wire::ToolCallStatus::Pending => acp::ToolStatus::Pending,
        wire::ToolCallStatus::InProgress => acp::ToolStatus::InProgress,
        wire::ToolCallStatus::Completed => acp::ToolStatus::Completed,
        wire::ToolCallStatus::Failed => acp::ToolStatus::Failed,
    };
    call.viz = record.viz.map(|tool| acp::VizLink {
        tool,
        child: record.child,
        environment: record.child_environment,
    });
    call.started_at = record.started_at;
    call.raw_input = record.input.map(Into::into);
    call.raw_output = record.output.map(Into::into);
    call.locations = record
        .locations
        .into_iter()
        .map(|path| acp::Location {
            path: path.into(),
            line: None,
        })
        .collect();
    call.content = record
        .diffs
        .into_iter()
        .map(|diff| {
            acp::ToolContent::Diff(acp::FileDiff {
                path: diff.path.into(),
                old_text: diff.old_text,
                new_text: diff.new_text,
            })
        })
        .collect();
    call
}
