//! Pure timeline formatting and work-group summary copy helpers.

use viz_client::protocol::UsageCost;

use super::{Item, ToolAction};

/// Formats a T3-style work group summary line such as
/// `"Changed 1 file, ran 1 command, and performed 2 other actions"`.
#[must_use]
pub fn work_group_summary(work: &[(&Item, bool)]) -> String {
    let mut changed_files = 0usize;
    let mut commands = 0usize;
    let mut reads = 0usize;
    let mut searches = 0usize;
    let mut fetches = 0usize;
    let mut thoughts = 0usize;
    let mut other = 0usize;
    for (item, _) in work {
        match item {
            Item::Thinking { text, .. } if !text.trim().is_empty() => thoughts += 1,
            Item::Tool(tool) => match ToolAction::of(tool) {
                ToolAction::Edit | ToolAction::Write | ToolAction::Delete | ToolAction::Move => {
                    changed_files += 1;
                }
                ToolAction::Command => commands += 1,
                ToolAction::Read => reads += 1,
                ToolAction::Search => searches += 1,
                ToolAction::Fetch => fetches += 1,
                ToolAction::Other => other += 1,
            },
            _ => {}
        }
    }
    let noun = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    let total_tools = changed_files + commands + reads + searches + fetches + other;
    if total_tools == 0 && thoughts > 0 {
        return if thoughts == 1 {
            "Thought through the problem".to_owned()
        } else {
            format!("{thoughts} reasoning steps")
        };
    }
    let mut clauses = Vec::new();
    if changed_files > 0 {
        clauses.push(format!(
            "changed {changed_files} {}",
            noun(changed_files, "file", "files")
        ));
    }
    if commands > 0 {
        clauses.push(format!(
            "ran {commands} {}",
            noun(commands, "command", "commands")
        ));
    }
    let minor = reads + searches + fetches + other;
    if clauses.is_empty() {
        if reads > 0 {
            clauses.push(format!("read {reads} {}", noun(reads, "file", "files")));
        }
        if searches > 0 {
            clauses.push(format!(
                "searched {searches} {}",
                noun(searches, "pattern", "patterns")
            ));
        }
        if fetches > 0 {
            clauses.push(format!(
                "fetched {fetches} {}",
                noun(fetches, "page", "pages")
            ));
        }
        if other > 0 {
            clauses.push(format!("used {other} {}", noun(other, "tool", "tools")));
        }
    } else if minor > 0 {
        clauses.push(format!(
            "performed {minor} other {}",
            noun(minor, "action", "actions")
        ));
    }
    let mut joined = match clauses.as_slice() {
        [] => "worked".to_owned(),
        [only] => only.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };
    if let Some(first) = joined.get(..1) {
        let upper = first.to_uppercase();
        joined.replace_range(..1, &upper);
    }
    joined
}

/// Formats elapsed or completed turn duration as `"5s"` or `"1m 12s"`.
#[must_use]
pub fn format_turn_duration(ms: i64) -> String {
    let secs = ms.max(0) / 1_000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Formats a Unix millisecond timestamp as `HH:MM` (UTC, deterministic).
#[must_use]
pub fn format_clock_hm(unix_ms: i64) -> Option<String> {
    if unix_ms <= 0 {
        return None;
    }
    let day_secs = (unix_ms / 1_000).rem_euclid(86_400);
    let hours = day_secs / 3_600;
    let mins = (day_secs % 3_600) / 60;
    Some(format!("{hours:02}:{mins:02}"))
}

/// Formats wire [`UsageCost`] cleanly (e.g. `"$0.04"`).
#[must_use]
pub fn format_cost(cost: &UsageCost) -> Option<String> {
    let amount = cost.amount.as_f64()?;
    if amount <= 0.0 {
        return None;
    }
    if cost.currency.eq_ignore_ascii_case("USD") {
        if amount < 0.01 {
            Some(format!("${amount:.4}"))
        } else {
            Some(format!("${amount:.2}"))
        }
    } else {
        Some(format!("{amount:.2} {}", cost.currency))
    }
}
