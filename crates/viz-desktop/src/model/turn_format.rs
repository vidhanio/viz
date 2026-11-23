//! Pure timeline formatting, diff-stat calculation, and work-group summary
//! copy helpers.

use std::collections::BTreeMap;

use viz_client::protocol::{ToolDiff, UsageCost};

use super::{Item, ToolAction};

/// Computes `(added, removed)` line counts for a wire [`ToolDiff`] purely.
#[must_use]
pub fn diff_line_stats(diff: &ToolDiff) -> (u32, u32) {
    let new_lines: Vec<&str> = diff.new_text.lines().collect();
    let Some(old_text) = diff.old_text.as_deref() else {
        return (u32::try_from(new_lines.len()).unwrap_or(u32::MAX), 0);
    };
    let old_lines: Vec<&str> = old_text.lines().collect();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "comparing trailing lines at independent old_end and new_end indices"
    )]
    while old_end > prefix && new_end > prefix && old_lines[old_end - 1] == new_lines[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
    }
    let a = &old_lines[prefix..old_end];
    let b = &new_lines[prefix..new_end];
    if a.is_empty() || b.is_empty() {
        return (
            u32::try_from(b.len()).unwrap_or(u32::MAX),
            u32::try_from(a.len()).unwrap_or(u32::MAX),
        );
    }
    let common = if a.len() * b.len() <= 65_536 {
        lcs_len(a, b)
    } else {
        multiset_common(a, b)
    };
    let added = b.len().saturating_sub(common);
    let removed = a.len().saturating_sub(common);
    (
        u32::try_from(added).unwrap_or(u32::MAX),
        u32::try_from(removed).unwrap_or(u32::MAX),
    )
}

fn lcs_len(a: &[&str], b: &[&str]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    let mut curr = vec![0usize; b.len() + 1];
    for &x in a {
        for (j, &y) in b.iter().enumerate() {
            curr[j + 1] = if x == y {
                prev[j] + 1
            } else {
                prev[j + 1].max(curr[j])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    prev[b.len()]
}

fn multiset_common<'a>(a: &[&'a str], b: &[&'a str]) -> usize {
    let mut counts: BTreeMap<&'a str, usize> = BTreeMap::new();
    for &line in a {
        *counts.entry(line).or_insert(0) += 1;
    }
    let mut common = 0;
    for &line in b {
        if let Some(slot) = counts.get_mut(line)
            && *slot > 0
        {
            *slot -= 1;
            common += 1;
        }
    }
    common
}

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
