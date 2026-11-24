//! Projects normalized tool records into timeline rows.

use viz_client::protocol::{FileOperation, ToolCall, ToolCallStatus, ToolPayload};

use super::{
    ToolAction,
    transcript::{Tool, ToolStatus},
};

const SUMMARY_CHARS: usize = 96;

/// Projects a normalized call into a timeline row.
#[expect(
    clippy::too_many_lines,
    reason = "projects the payload and common tool fields into one row"
)]
pub(super) fn project_tool(turn: Option<&str>, record: &ToolCall, hydrated: bool) -> Tool {
    let (action, detail, input, timeout_ms, search_matches) = match &record.payload {
        ToolPayload::Execute {
            command,
            timeout_ms,
        } => (
            ToolAction::Command,
            command.as_deref(),
            command.clone(),
            *timeout_ms,
            Vec::new(),
        ),
        ToolPayload::Read { .. } => (ToolAction::Read, None, None, None, Vec::new()),
        ToolPayload::Edit { operation, .. } => {
            let action = match operation {
                FileOperation::Write => ToolAction::Write,
                FileOperation::Edit => ToolAction::Edit,
                FileOperation::Delete => ToolAction::Delete,
                FileOperation::Move => ToolAction::Move,
            };
            (action, None, None, None, Vec::new())
        }
        ToolPayload::Search { query, matches } => (
            ToolAction::Search,
            query.as_deref(),
            None,
            None,
            matches.clone(),
        ),
        ToolPayload::Fetch { url, matches } => (
            ToolAction::Fetch,
            url.as_deref(),
            None,
            None,
            matches.clone(),
        ),
        ToolPayload::Mcp { summary } | ToolPayload::Other { summary } => (
            ToolAction::Other,
            summary.as_deref(),
            None,
            None,
            Vec::new(),
        ),
    };
    let target = record.payload.targets().first();
    let summary = first_line(
        detail
            .or_else(|| target.and_then(|target| target.path.to_str()))
            .or_else(|| (record.title != record.id).then_some(record.title.as_str()))
            .unwrap_or_default(),
        SUMMARY_CHARS,
    );
    let output = if hydrated {
        record.full_output_text()
    } else {
        record.output_text()
    }
    .unwrap_or_default();
    let exit = record.exit.as_ref().or_else(|| {
        record
            .terminals
            .iter()
            .rev()
            .find_map(|terminal| terminal.exit.as_ref())
    });
    let exit_code = exit
        .and_then(|exit| exit.code)
        .and_then(|code| i32::try_from(code).ok());
    let exit_signal = exit.and_then(|exit| exit.signal.clone());
    let terminal_truncated = record.terminals.iter().any(|terminal| terminal.truncated);
    let status = match record.status {
        ToolCallStatus::Pending => ToolStatus::Pending,
        ToolCallStatus::InProgress => ToolStatus::Running,
        ToolCallStatus::Completed if exit_code.is_some_and(|code| code != 0) => ToolStatus::Failed,
        ToolCallStatus::Completed => ToolStatus::Done,
        ToolCallStatus::Failed => ToolStatus::Failed,
    };
    Tool {
        key: format!("acp-{}-{}", turn.unwrap_or_default(), record.id),
        call_id: record.id.clone(),
        entry: None,
        name: record
            .name
            .clone()
            .or_else(|| record.kind.clone())
            .unwrap_or_else(|| "tool".into()),
        action,
        target: record.payload.targets().first().cloned(),
        raw_name: record.name.clone(),
        title: record.title.clone(),
        summary,
        input,
        pending_input: (status == ToolStatus::Running)
            .then(|| record.raw_input.as_ref().map(serde_json::Value::to_string))
            .flatten(),
        raw_input_pretty: record.arguments.clone(),
        timeout_ms,
        started_at: record.started_at,
        ended_at: record.ended_at,
        status,
        exit_code,
        exit_signal,
        output,
        terminal_truncated,
        full: None,
        review: record.review.clone(),
        task_id: record.task_id.clone(),
        child: record.viz.as_ref().and_then(|viz| viz.child),
        child_environment: record.viz.as_ref().and_then(|viz| viz.environment.clone()),
        viz: record.viz.as_ref().map(|viz| viz.tool.clone()),
        diffs: record.diffs().cloned().collect(),
        search_matches,
    }
}

/// The first non-empty line of `text`, cut to `max` characters, with an
/// ellipsis when anything was left out.
fn first_line(text: &str, max: usize) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let cut = cap_head(lines.next().unwrap_or_default(), max);
    if lines.next().is_some() && !cut.ends_with('…') {
        format!("{cut} …")
    } else {
        cut
    }
}

/// `text`'s first `max` characters, with an ellipsis if it was longer.
fn cap_head(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}
