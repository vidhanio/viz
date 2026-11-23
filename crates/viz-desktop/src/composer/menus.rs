//! The `@`, `/`, `$`, and `#` completion popup: a `rune_kit::PopupList`
//! anchored above the composer card while focus and arrow keys stay in the
//! draft field.

use rune::{El, icon, text};
use rune_kit::{Badge, BadgeVariant, Placement, PopupItem, PopupList};
use viz_client::protocol::{FileSearchKind, FileSearchMatch, GitHubItemKind};

use crate::{
    app::Desktop,
    model::{
        CompletionKind, GitHubEntry, MentionEntry, SkillEntry, SlashEntry, matching_commands,
        matching_github, matching_mentions, matching_skills,
    },
    theme::{ICON_MD, Theme},
    ui,
    update::{ComposerMsg, Msg},
};

/// The composer card the popup anchors to.
const ANCHOR: &str = "composer-card";
/// The popup's width.
const WIDTH: f32 = 420.0;
/// Maximum scrollable height of the popup.
const MAX_HEIGHT: f32 = 288.0;

/// Mounts the completion popup (each item's id is its row index, as
/// [`ComposerMsg::ChooseCompletion`] takes) into `layer` while the draft field
/// holds a `@`, `/`, `$`, or `#` token.
pub(super) fn popup(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    let completion = &m.composer.completion;
    let Some(token) = completion
        .token
        .as_ref()
        .filter(|_| !completion.dismissed && s.widgets.draft.focused)
    else {
        return;
    };
    let (heading, items, empty): (_, Vec<_>, _) = match token.kind {
        CompletionKind::Slash => (
            "Commands",
            matching_commands(m, &token.query, token.range.start == 0)
                .iter()
                .enumerate()
                .map(|(index, entry)| slash_item(index, entry, t))
                .collect(),
            "No matching commands.",
        ),
        CompletionKind::Mention => {
            let empty = match &completion.files {
                None if m.selection.selected.is_some() => "Searching files…",
                Some(Err(_)) => "File search failed.",
                _ => "No matching files or mentions.",
            };
            (
                "Mentions",
                matching_mentions(m, &token.query)
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| mention_item(index, entry, t))
                    .collect(),
                empty,
            )
        }
        CompletionKind::Skill => (
            "Skills",
            matching_skills(m, &token.query)
                .iter()
                .enumerate()
                .map(|(index, entry)| skill_item(index, entry, t))
                .collect(),
            "No skills found. Try / to browse provider commands.",
        ),
        CompletionKind::PullRequest => {
            let empty = match &completion.github {
                None if m.project().is_some_and(|p| p.git) => "Searching pull requests & issues…",
                Some(Err(_)) => "GitHub search unavailable.",
                _ => "No matching pull requests or issues.",
            };
            (
                "Pull requests",
                matching_github(m, &token.query)
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| github_item(index, entry, t))
                    .collect(),
                empty,
            )
        }
    };
    let active = completion.highlighted.min(items.len().saturating_sub(1));
    PopupList::new("composer-completion", ANCHOR)
        .placement(Placement::TopStart)
        .open(true)
        .width(WIDTH)
        .max_height(MAX_HEIGHT)
        .label(heading)
        .empty_message(empty)
        .items(items)
        .active(Some(active))
        .on_active(|s: &mut Desktop, index| {
            s.dispatch_local(Msg::Composer(ComposerMsg::CompletionHover(index)));
        })
        .on_choose(|s: &mut Desktop, id, tx| {
            let Ok(index) = id.parse() else {
                return;
            };
            let cursor = s.draft_cursor();
            s.dispatch(
                Msg::Composer(ComposerMsg::ChooseCompletion { index, cursor }),
                tx,
            );
        })
        .build(&t.overlay, layer);
}

fn glyph(glyph: &'static rune::Icon, color: rune::Color) -> El {
    icon(glyph, ICON_MD).color(color).a11y_hidden()
}

fn slash_item(index: usize, entry: &SlashEntry, t: &Theme) -> PopupItem {
    let item = PopupItem::new(index.to_string(), format!("/{}", entry.name))
        .description(&entry.description);
    match entry.hint.as_deref() {
        Some(hint) => item.trailing(text(hint).font(t.mono_small).color(t.faint).a11y_hidden()),
        None => item,
    }
}

fn mention_item(index: usize, entry: &MentionEntry, t: &Theme) -> PopupItem {
    match entry {
        MentionEntry::File(file) => file_item(index, file, t),
        MentionEntry::Thread { title, project, .. } => {
            let item =
                PopupItem::new(index.to_string(), title).leading(glyph(&ui::THREAD, t.accent_text));
            match project {
                Some(project) => item.description(project),
                None => item,
            }
        }
        MentionEntry::Environment { id, reachable } => PopupItem::new(index.to_string(), id)
            .leading(glyph(&ui::SERVER, t.info))
            .trailing(
                Badge::new(if *reachable { "Online" } else { "Offline" })
                    .variant(if *reachable {
                        BadgeVariant::Neutral
                    } else {
                        BadgeVariant::Subtle
                    })
                    .build(&t.k)
                    .a11y_hidden(),
            ),
        MentionEntry::Model { value, label } => PopupItem::new(index.to_string(), label)
            .description(value)
            .leading(glyph(&ui::CPU, t.delegated)),
    }
}

fn file_item(index: usize, file: &FileSearchMatch, t: &Theme) -> PopupItem {
    let path = &file.path;
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) if !name.is_empty() => (Some(dir), name),
        _ => (None, path.as_str()),
    };
    let leading = if file.kind == FileSearchKind::Directory {
        glyph(&ui::FOLDER, t.info)
    } else {
        glyph(rune_kit::file_icon(path), t.muted)
    };
    let item = PopupItem::new(index.to_string(), name).leading(leading);
    match dir {
        Some(dir) => item.description(dir),
        None => item,
    }
}

fn skill_item(index: usize, entry: &SkillEntry, t: &Theme) -> PopupItem {
    PopupItem::new(index.to_string(), format!("${}", entry.name))
        .description(&entry.description)
        .leading(glyph(&ui::SPARKLES, t.steer))
        .trailing(Badge::new(entry.origin.label()).build(&t.k).a11y_hidden())
}

fn github_item(index: usize, entry: &GitHubEntry, t: &Theme) -> PopupItem {
    let item = &entry.item;
    let state_lower = item.state.to_ascii_lowercase();
    let (mark, color) = match (item.kind, state_lower.as_str()) {
        (GitHubItemKind::PullRequest, "merged") => (&ui::GIT_MERGE, t.delegated),
        (GitHubItemKind::PullRequest, "draft") => (&ui::PR_DRAFT, t.muted),
        (GitHubItemKind::PullRequest, "closed") => (&ui::PR_CLOSED, t.danger),
        (GitHubItemKind::PullRequest, _) => (&ui::PR_OPEN, t.success),
        (GitHubItemKind::Issue, "closed") => (&ui::CIRCLE_CHECK, t.delegated),
        (GitHubItemKind::Issue, _) => (&ui::CIRCLE_DOT, t.success),
    };
    let badge = if entry.linked {
        Badge::new("Linked").variant(BadgeVariant::Accent)
    } else {
        Badge::new(item.state.as_str())
    };
    PopupItem::new(
        index.to_string(),
        format!("#{} {}", item.number, item.title),
    )
    .leading(glyph(mark, color))
    .trailing(badge.build(&t.k).a11y_hidden())
}
