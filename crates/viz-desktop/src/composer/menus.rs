//! Tucked `@`, `/`, `$`, and `#` completion drawers anchored seamlessly behind
//! the composer's top edge.

use rune::{Color, El, ScrollFade, ScrollSpec, Span, col, icon, rich, row, text};
use viz_client::protocol::{FileSearchKind, FileSearchMatch, GitHubItemKind};

use crate::{
    app::Desktop,
    model::{
        CompletionKind, GitHubEntry, MentionEntry, SkillEntry, SlashEntry, matching_commands,
        matching_github, matching_mentions, matching_skills,
    },
    theme::{HAIRLINE, ICON, ICON_MD, ICON_SMALL, RADIUS_SM, S6, S8, S10, S12, S16, Theme},
    ui::{self, BadgeSize, Tone},
    update::{ComposerMsg, Msg},
};

const POPUP: &str = "composer-completion";
const LIST: &str = "completion-list";
/// Maximum scrollable height of the completion drawer (`max-h-72` = `288pt`).
const DRAWER_MAX_HEIGHT: f32 = 288.0;
/// Compact completion item row height (`30pt`).
const ROW_HEIGHT: f32 = 30.0;
/// Maximum width of a slash command's argument hint.
const HINT_MAX_WIDTH: f32 = 136.0;

/// Attached completion drawer tucked flush behind the composer's top edge when
/// typing `@`, `/`, `$`, or `#`.
pub(super) fn drawer(s: &Desktop, t: &Theme) -> Option<El> {
    let m = &s.model;
    let completion = &m.composer.completion;
    let token = completion
        .token
        .as_ref()
        .filter(|_| !completion.dismissed && s.widgets.draft.focused)?;

    let (heading, rows, empty_message) = match token.kind {
        CompletionKind::Slash => {
            let matched = matching_commands(m, &token.query, token.range.start == 0);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, entry)| slash_row(entry, index, index == active, t))
                .collect();
            ("Commands", rows, "No matching commands.")
        }
        CompletionKind::Mention => {
            let matched = matching_mentions(m, &token.query);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, entry)| mention_row(entry, index, index == active, t))
                .collect();
            let empty = match &completion.files {
                None if m.selection.selected.is_some() => "Searching files…",
                Some(Err(_)) => "File search failed.",
                _ => "No matching files or mentions.",
            };
            ("Mentions", rows, empty)
        }
        CompletionKind::Skill => {
            let matched = matching_skills(m, &token.query);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, entry)| skill_row(entry, index, index == active, t))
                .collect();
            (
                "Skills",
                rows,
                "No skills found. Try / to browse provider commands.",
            )
        }
        CompletionKind::PullRequest => {
            let matched = matching_github(m, &token.query);
            let active = completion.highlighted.min(matched.len().saturating_sub(1));
            let rows: Vec<_> = matched
                .iter()
                .enumerate()
                .map(|(index, entry)| github_row(entry, index, index == active, t))
                .collect();
            let empty = match &completion.github {
                None if m.project().is_some_and(|p| p.git) => "Searching pull requests & issues…",
                Some(Err(_)) => "GitHub search unavailable.",
                _ => "No matching pull requests or issues.",
            };
            ("Pull requests", rows, empty)
        }
    };

    let mut list = col()
        .key(LIST)
        .role("listbox")
        .label(heading)
        .w_full()
        .max_h(DRAWER_MAX_HEIGHT)
        .min_h(0.0)
        .padding(S6)
        .gap(2.0)
        .instant_changes()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .scroll_fade(ScrollFade::strength(S16).linear());

    if rows.is_empty() {
        list = list.child(
            text(empty_message)
                .role("status")
                .font(t.small)
                .color(t.faint)
                .px(S10)
                .py(S12),
        );
    } else {
        list = list.children(rows);
    }

    Some(
        ui::tucked_drawer(POPUP, 25, list, t)
            .role("group")
            .label(heading),
    )
}

fn base_row(
    key: String,
    label: &str,
    description: Option<&str>,
    index: usize,
    highlighted: bool,
    t: &Theme,
) -> El {
    let mut item = row()
        .key(key)
        .role("option")
        .label(label)
        .focus_selected(highlighted)
        .preserve_focus_on_press(true)
        .w_full()
        .h(ROW_HEIGHT)
        .px(S10)
        .gap(S8)
        .center_y()
        .radius(RADIUS_SM)
        .fill(if highlighted {
            t.selected
        } else {
            Color::TRANSPARENT
        })
        .on_hover(move |s: &mut Desktop| {
            s.dispatch_local(Msg::Composer(ComposerMsg::CompletionHover(index)));
        })
        .on_press_tx(move |s: &mut Desktop, tx| {
            let cursor = s.draft_cursor();
            s.dispatch(
                Msg::Composer(ComposerMsg::ChooseCompletion { index, cursor }),
                tx,
            );
        });
    if let Some(desc) = description {
        item = item.description(desc);
    }
    if highlighted {
        item = item.focus_ring_for("draft", HAIRLINE, -HAIRLINE, t.accent);
    }
    item
}

fn slash_row(entry: &SlashEntry, index: usize, highlighted: bool, t: &Theme) -> El {
    let label = format!("/{}", entry.name);
    base_row(
        format!("slash-{}", entry.name),
        &label,
        Some(&entry.description),
        index,
        highlighted,
        t,
    )
    .child(
        text(&label)
            .font(t.panel_row)
            .color(if highlighted { t.text_strong } else { t.text })
            .a11y_hidden(),
    )
    .child(
        text(&entry.description)
            .font(t.small)
            .color(t.muted)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate()
            .a11y_hidden(),
    )
    .children(entry.hint.as_deref().map(|hint| {
        text(hint)
            .font(t.mono_small)
            .color(t.faint)
            .max_w(HINT_MAX_WIDTH)
            .truncate()
            .a11y_hidden()
    }))
}

fn mention_row(entry: &MentionEntry, index: usize, highlighted: bool, t: &Theme) -> El {
    match entry {
        MentionEntry::File(file) => file_row(file, index, highlighted, t),
        MentionEntry::Thread { id, title, project } => {
            let mut item = base_row(
                format!("mention-thread-{}", id.0),
                title,
                project.as_deref(),
                index,
                highlighted,
                t,
            )
            .child(
                icon(&ui::THREAD, ICON_MD)
                    .color(t.accent_text)
                    .a11y_hidden(),
            )
            .child(
                text(title)
                    .font(t.panel_row)
                    .color(t.text_strong)
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .truncate()
                    .a11y_hidden(),
            );
            if let Some(project) = project {
                item = item.child(text(project).font(t.small).color(t.faint).a11y_hidden());
            }
            item
        }
        MentionEntry::Environment { id, reachable } => {
            base_row(format!("mention-env-{id}"), id, None, index, highlighted, t)
                .child(icon(&ui::SERVER, ICON_MD).color(t.info).a11y_hidden())
                .child(
                    text(id)
                        .font(t.panel_row)
                        .color(t.text_strong)
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .truncate()
                        .a11y_hidden(),
                )
                .child(
                    ui::badge(if *reachable { "Online" } else { "Offline" })
                        .tone(if *reachable {
                            Tone::Info
                        } else {
                            Tone::Neutral
                        })
                        .size(BadgeSize::Nano)
                        .build(t)
                        .a11y_hidden(),
                )
        }
        MentionEntry::Model { value, label } => base_row(
            format!("mention-model-{value}"),
            label,
            Some(value),
            index,
            highlighted,
            t,
        )
        .child(icon(&ui::CPU, ICON_MD).color(t.delegated).a11y_hidden())
        .child(
            text(label)
                .font(t.panel_row)
                .color(t.text_strong)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            text(value)
                .font(t.mono_micro)
                .color(t.faint)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_middle()
                .a11y_hidden(),
        ),
    }
}

fn file_row(file: &FileSearchMatch, index: usize, highlighted: bool, t: &Theme) -> El {
    let path = &file.path;
    let (dir, name, name_char_offset) = match path.rsplit_once('/') {
        Some((dir, name)) if !name.is_empty() => (Some(dir), name, dir.chars().count() + 1),
        _ => (None, path.as_str(), 0),
    };
    let glyph = if file.kind == FileSearchKind::Directory {
        &ui::FOLDER
    } else {
        rune_kit::file_icon(path)
    };
    let icon_color = if file.kind == FileSearchKind::Directory {
        t.info
    } else if highlighted {
        t.accent_text
    } else {
        t.muted
    };
    let name_el = highlighted_basename(name, name_char_offset, &file.positions, t);
    base_row(
        format!("mention-file-{path}"),
        path,
        dir,
        index,
        highlighted,
        t,
    )
    .child(icon(glyph, ICON).color(icon_color).a11y_hidden())
    .child(name_el)
    .children(dir.map(|dir| {
        text(dir)
            .font(t.small)
            .color(t.muted)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate_middle()
            .a11y_hidden()
    }))
}

fn highlighted_basename(name: &str, offset: usize, positions: &[u32], t: &Theme) -> El {
    let mut spans = Vec::new();
    for (char_idx, (byte_idx, ch)) in name.char_indices().enumerate() {
        let global = offset + char_idx;
        if positions.iter().any(|&p| p as usize == global) {
            spans.push(Span {
                color: Some(t.accent_text),
                ..Span::new(byte_idx, byte_idx + ch.len_utf8())
            });
        }
    }
    if spans.is_empty() {
        text(name)
            .font(t.panel_row)
            .color(t.text_strong)
            .max_w(300.0)
            .shrink()
            .truncate()
            .a11y_hidden()
    } else {
        rich(name, spans)
            .font(t.panel_row)
            .color(t.text_strong)
            .max_w(300.0)
            .shrink()
            .truncate()
            .a11y_hidden()
    }
}

fn skill_row(entry: &SkillEntry, index: usize, highlighted: bool, t: &Theme) -> El {
    let label = format!("${}", entry.name);
    base_row(
        format!("skill-{}", entry.name),
        &label,
        Some(&entry.description),
        index,
        highlighted,
        t,
    )
    .child(
        icon(&ui::SPARKLES, ICON_SMALL + 1.0)
            .color(t.steer)
            .a11y_hidden(),
    )
    .child(
        text(&entry.name)
            .font(t.panel_row)
            .color(t.text_strong)
            .a11y_hidden(),
    )
    .child(
        text(&entry.description)
            .font(t.small)
            .color(t.muted)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate()
            .a11y_hidden(),
    )
    .child(
        ui::badge(entry.origin.label())
            .tone(Tone::Neutral)
            .size(BadgeSize::Nano)
            .build(t)
            .a11y_hidden(),
    )
}

fn github_row(entry: &GitHubEntry, index: usize, highlighted: bool, t: &Theme) -> El {
    let item = &entry.item;
    let state_lower = item.state.to_ascii_lowercase();
    let (glyph, tone, color) = match (item.kind, state_lower.as_str()) {
        (GitHubItemKind::PullRequest, "merged") => (&ui::GIT_MERGE, Tone::Delegated, t.delegated),
        (GitHubItemKind::PullRequest, "draft") => (&ui::PR_DRAFT, Tone::Neutral, t.muted),
        (GitHubItemKind::PullRequest, "closed") => (&ui::PR_CLOSED, Tone::Danger, t.danger),
        (GitHubItemKind::PullRequest, _) => (&ui::PR_OPEN, Tone::Success, t.success),
        (GitHubItemKind::Issue, "closed") => (&ui::CIRCLE_CHECK, Tone::Delegated, t.delegated),
        (GitHubItemKind::Issue, _) => (&ui::CIRCLE_DOT, Tone::Success, t.success),
    };
    let num = format!("#{}", item.number);
    let badge_text = if entry.linked {
        "Linked"
    } else {
        item.state.as_str()
    };
    let badge_tone = if entry.linked { Tone::Accent } else { tone };
    base_row(
        format!("gh-{:?}-{}", item.kind, item.number),
        &format!("{num} {}", item.title),
        Some(&item.state),
        index,
        highlighted,
        t,
    )
    .child(icon(glyph, ICON_MD).color(color).a11y_hidden())
    .child(
        text(&num)
            .font(t.mono_small)
            .color(t.text_strong)
            .a11y_hidden(),
    )
    .child(
        text(&item.title)
            .font(t.panel_row)
            .color(t.text)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate()
            .a11y_hidden(),
    )
    .child(
        ui::badge(badge_text)
            .tone(badge_tone)
            .size(BadgeSize::Nano)
            .build(t)
            .a11y_hidden(),
    )
}
