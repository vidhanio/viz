//! Pure ranking and filtering matchers for the composer's tucked `/`, `@`,
//! `$`, and `#` completion drawers.

use viz_client::protocol::{
    CommandSource, EnvironmentStatus, GitHubItem, GitHubItemKind, GitHubPullState,
};

use super::{
    Model, OptionRole,
    completion::{
        BuiltinCommand, GitHubEntry, MentionEntry, SkillEntry, SkillOrigin, SlashEntry,
        available_commands,
    },
    option_role, thread_title,
};

/// Ranked `/` slash commands (built-in commands at prompt start + ACP harness
/// commands + skills exposed as `/skill:<name>`).
#[must_use]
pub fn matching_commands(model: &Model, query: &str, at_start: bool) -> Vec<SlashEntry> {
    let mut out = Vec::new();
    let harness_commands = available_commands(model);
    if at_start {
        let builtins = [
            (
                "model",
                "Switch response model for this thread",
                BuiltinCommand::Model,
            ),
            (
                "clear",
                "Start a fresh session; earlier messages stay visible",
                BuiltinCommand::Clear,
            ),
            (
                "compact",
                "Summarize the conversation and reduce context usage",
                BuiltinCommand::Compact,
            ),
        ];
        for (name, description, builtin) in builtins {
            if let Some(score) = score_command(name, description, query) {
                let is_harness_compact = builtin == BuiltinCommand::Compact
                    && harness_commands
                        .iter()
                        .any(|cmd| cmd.name.eq_ignore_ascii_case("compact"));
                out.push(SlashEntry {
                    name: name.to_owned(),
                    description: description.to_owned(),
                    hint: None,
                    builtin: (!is_harness_compact).then_some(builtin),
                    score,
                });
            }
        }
    }
    for command in harness_commands {
        let (display_name, desc) = match command.source {
            CommandSource::Skill => (
                format!("skill:{}", command.name),
                command.description.as_str(),
            ),
            CommandSource::Command | CommandSource::Mcp => {
                (command.name.clone(), command.description.as_str())
            }
        };
        if at_start
            && out
                .iter()
                .any(|existing| existing.name.eq_ignore_ascii_case(&display_name))
        {
            continue;
        }
        if let Some(score) = score_command(&display_name, desc, query) {
            out.push(SlashEntry {
                name: display_name,
                description: command.description.clone(),
                hint: command.hint.clone(),
                builtin: None,
                score: score.saturating_add(1),
            });
        }
    }
    out.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Ranked `$` skills advertised by the active thread or harness.
#[must_use]
pub fn matching_skills(model: &Model, query: &str) -> Vec<SkillEntry> {
    let mut out = Vec::new();
    for command in available_commands(model) {
        if command.source != CommandSource::Skill {
            continue;
        }
        if let Some(score) = score_command(&command.name, &command.description, query) {
            out.push(SkillEntry {
                name: command.name.clone(),
                description: command.description.clone(),
                origin: infer_skill_origin(&command.name, &command.description),
                score,
            });
        }
    }
    out.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Ranked `@` mentions combining workspace files, recent threads, peer
/// environments, and active-harness models.
#[must_use]
pub fn matching_mentions(model: &Model, query: &str) -> Vec<MentionEntry> {
    let mut out = Vec::new();
    let query_lower = query.trim().to_ascii_lowercase();
    let completion = &model.composer.completion;
    if let Some(Ok(files)) = &completion.files {
        let exact = completion.files_query.as_deref() == Some(query);
        if exact || query_lower.is_empty() {
            for matched in files {
                out.push(MentionEntry::File(matched.clone()));
            }
        } else {
            let mut scored: Vec<(u32, &viz_client::protocol::FileSearchMatch)> = files
                .iter()
                .filter_map(|matched| {
                    let name = matched
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(matched.path.as_str());
                    let score = score_text(name, &query_lower)
                        .or_else(|| score_text(&matched.path, &query_lower).map(|s| s + 10))?;
                    Some((score, matched))
                })
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
            for (_, matched) in scored {
                out.push(MentionEntry::File(matched.clone()));
            }
        }
    }
    let max_threads = if out.is_empty() { 8 } else { 4 };
    let mut added_threads = 0usize;
    for thread in model.cache().threads() {
        if Some(thread.id) == model.selection.selected || thread.lifecycle.archived {
            continue;
        }
        let title = thread_title(thread);
        if !query_lower.is_empty() && score_text(title, &query_lower).is_none() {
            continue;
        }
        let project = thread
            .project
            .as_deref()
            .and_then(|path| model.cache().project(path))
            .map(|p| p.name.clone());
        out.push(MentionEntry::Thread {
            id: thread.id,
            title: title.to_owned(),
            project,
        });
        added_threads += 1;
        if added_threads >= max_threads {
            break;
        }
    }
    if model.connection.environments.len() >= 2 {
        for env in &model.connection.environments {
            if !query_lower.is_empty() && score_text(&env.id, &query_lower).is_none() {
                continue;
            }
            out.push(MentionEntry::Environment {
                id: env.id.clone(),
                reachable: env.status != EnvironmentStatus::Unreachable,
            });
        }
    }
    if !query_lower.is_empty() {
        let options = active_options(model);
        if let Some(model_opt) = options
            .iter()
            .find(|opt| option_role(opt) == OptionRole::Model)
        {
            for choice in &model_opt.choices {
                if score_text(&choice.name, &query_lower).is_some()
                    || score_text(&choice.value, &query_lower).is_some()
                {
                    out.push(MentionEntry::Model {
                        value: choice.value.clone(),
                        label: choice.name.clone(),
                    });
                    if out.len() >= 32 {
                        break;
                    }
                }
            }
        }
    }
    out
}

/// Ranked `#` GitHub pull requests and issues, placing any PR linked to the
/// active thread first.
#[must_use]
pub fn matching_github(model: &Model, query: &str) -> Vec<GitHubEntry> {
    let mut out = Vec::new();
    let q = query.trim().trim_start_matches('#').to_ascii_lowercase();
    if let Some(pr) = model
        .selection
        .selected
        .and_then(|id| model.summary(id))
        .and_then(|summary| summary.pull_request.as_ref())
    {
        let number_str = pr.number.to_string();
        if q.is_empty() || number_str.contains(&q) || pr.title.to_ascii_lowercase().contains(&q) {
            let state = if pr.draft {
                "draft"
            } else {
                match pr.state {
                    GitHubPullState::Open => "open",
                    GitHubPullState::Merged => "merged",
                    GitHubPullState::Closed => "closed",
                }
            };
            out.push(GitHubEntry {
                item: GitHubItem {
                    kind: GitHubItemKind::PullRequest,
                    number: pr.number,
                    title: pr.title.clone(),
                    url: pr.url.clone(),
                    state: state.to_owned(),
                },
                linked: true,
            });
        }
    }
    if let Some(Ok(items)) = &model.composer.completion.github {
        for item in items {
            if out.iter().any(|existing| {
                existing.item.number == item.number && existing.item.kind == item.kind
            }) {
                continue;
            }
            out.push(GitHubEntry {
                item: item.clone(),
                linked: false,
            });
        }
    }
    out
}

fn active_options(model: &Model) -> &[viz_client::protocol::ConfigOption] {
    if let Some(state) = model.thread_state() {
        return &state.options;
    }
    model
        .selection
        .new_thread
        .as_ref()
        .and_then(|draft| draft.active_harness(model.harnesses()))
        .or_else(|| model.harnesses().iter().find(|h| h.default))
        .map_or(&[], |harness| harness.options.as_slice())
}

fn infer_skill_origin(name: &str, description: &str) -> SkillOrigin {
    let lower_desc = description.to_ascii_lowercase();
    let lower_name = name.to_ascii_lowercase();
    if lower_desc.contains("repo") || lower_name.starts_with("repo:") {
        SkillOrigin::Repo
    } else if lower_desc.contains("project") || lower_name.starts_with("project:") {
        SkillOrigin::Project
    } else if lower_desc.contains("personal") || lower_desc.contains("user") {
        SkillOrigin::Personal
    } else if lower_desc.contains("system") {
        SkillOrigin::System
    } else {
        SkillOrigin::Provider
    }
}

/// Scores a slash command or skill against `query` following `composer.md` §4:
/// exact (`0`) < prefix (`2`) < word-boundary (`4`) < substring (`6`) <
/// description match (`20`) < fuzzy (`100`).
#[must_use]
pub fn score_command(name: &str, description: &str, query: &str) -> Option<u32> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if let Some(score) = score_text(name, &q) {
        return Some(score);
    }
    let desc_lower = description.to_ascii_lowercase();
    if desc_lower.contains(&q) {
        return Some(20);
    }
    fuzzy_match(&name.to_ascii_lowercase(), &q).then_some(100)
}

fn score_text(candidate: &str, query_lower: &str) -> Option<u32> {
    let lower = candidate.to_ascii_lowercase();
    if lower == query_lower {
        Some(0)
    } else if lower.starts_with(query_lower) {
        Some(2)
    } else if lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word.starts_with(query_lower))
    {
        Some(4)
    } else if lower.contains(query_lower) {
        Some(6)
    } else if fuzzy_match(&lower, query_lower) {
        Some(100)
    } else {
        None
    }
}

fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    for target in needle.chars() {
        if !chars.any(|ch| ch == target) {
            return false;
        }
    }
    true
}
