//! Docked structured diff viewer and checkpoint controls.

use std::fmt::Write as _;

use rune::{El, col, icon, row, text};
use rune_kit::{
    ButtonVariant, FileTree, IconButtonVariant, Menu, MenuItem, Placement, Tree, TreeItem,
    diff::{self, DiffMode, DiffView, FileDiff, FileStatus},
};
use viz_client::protocol::{GitDiffFile, GitDiffTarget, GitFileKind, GitLineStats, RunId};

use crate::{
    app::Desktop,
    effects::git::Request,
    header::CHAT_HEADER_HEIGHT,
    model::git::{Dialog, Popup, default_branch},
    theme::{ICON_SMALL, S2, S4, S6, S8, S12, Size, Theme},
    ui,
    update::GitMsg,
};

const SCOPE: &str = "diff-scope";

pub fn stat(stats: GitLineStats, t: &Theme) -> El {
    ui::diff_stat_rolling("git-stat", count(stats.added), count(stats.removed), t)
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn sync(s: &mut Desktop) {
    let current = &s.model.git.diff;
    if match (&s.widgets.git_diff_source, current) {
        (Some(previous), Some(now)) => std::sync::Arc::ptr_eq(previous, now),
        (None, None) => true,
        _ => false,
    } {
        return;
    }
    let files: Vec<_> = current
        .iter()
        .flat_map(|diff| diff.files.iter().map(file))
        .collect();
    let mut tree = FileTree::new().key("diff-files").build(
        files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path(), (i, count(f.additions()), count(f.deletions())))),
    );
    tree.expand_all();
    s.widgets.git_diff_tree = tree;
    s.widgets.git_diff.set_files(files);
    s.widgets.git_diff_source = current.clone();
}

fn file(wire: &GitDiffFile) -> FileDiff {
    let mut patch = String::from("diff --git a/file b/file\n--- a/file\n+++ a/file\n");
    for hunk in &wire.hunks {
        let _ = writeln!(
            patch,
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        );
        for line in &hunk.lines {
            match line.origin {
                ' ' | '+' | '-' => {
                    patch.push(line.origin);
                    patch.push_str(&line.content);
                    if !line.content.ends_with('\n') {
                        patch.push('\n');
                    }
                }
                '=' | '>' | '<' => patch.push_str("\\ No newline at end of file\n"),
                _ => {}
            }
        }
    }
    let mut file = diff::parse(&patch)
        .into_iter()
        .next()
        .unwrap_or_else(|| FileDiff::from_texts("file", "", ""));
    file.old_path.clone_from(&wire.old_path);
    file.new_path.clone_from(&wire.new_path);
    file.binary = wire.binary;
    file.status = match wire.kind {
        GitFileKind::Added | GitFileKind::Untracked => FileStatus::Added,
        GitFileKind::Deleted => FileStatus::Deleted,
        GitFileKind::Renamed => FileStatus::Renamed,
        GitFileKind::Copied => FileStatus::Copied,
        _ => FileStatus::Modified,
    };
    file
}

pub fn view(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let mut panel = col()
        .key("git-diff-panel")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(toolbar(s, t, layer))
        .child(ui::rule(t));
    if matches!(git.target, GitDiffTarget::Turn { .. }) && git.diff.is_some() {
        panel = panel.child(
            row().w_full().px(S12).py(S6).center_y().child(
                rune_kit::Button::new("Restore checkpoint…")
                    .variant(ButtonVariant::Ghost)
                    .control_size(Size::Small.kit())
                    .disabled(s.model.busy())
                    .on_press_tx(|s: &mut Desktop, tx| {
                        s.widgets.git_body = rune::TextEdit::new("");
                        super::dispatch(s, GitMsg::Dialog(Some(Dialog::Restore)), tx);
                    })
                    .build(&t.k),
            ),
        );
    }
    if git
        .diff
        .as_ref()
        .is_some_and(|diff| diff.truncated || diff.files.iter().any(|f| f.truncated))
    {
        panel = panel.child(
            text("This preview is incomplete.")
                .font(t.small)
                .color(t.muted)
                .px(S12)
                .py(S6),
        );
    }
    if let Some(error) = &git.diff_error {
        return panel.child(empty_panel("Couldn’t load diff", Some(error.as_str()), t));
    }
    if s.widgets.git_diff.files().is_empty() {
        return panel.child(if git.diff.is_none() {
            col()
                .grow(1.0)
                .w_full()
                .center()
                .padding(S12)
                .child(ui::loading_state(
                    "Loading diff…",
                    None,
                    ui::LoadingDelay::Immediate,
                    t,
                ))
        } else {
            empty_panel(
                "No changes in this scope",
                Some("Switch scopes above to compare staged, branch, or thread changes."),
                t,
            )
        });
    }
    let diff = DiffView::new(
        |s: &Desktop| &s.widgets.git_diff,
        |s| &mut s.widgets.git_diff,
    )
    .key("git-diff-view")
    .build(s, &t.k)
    .grow(1.0)
    .min_w(0.0);
    if !s.widgets.git_tree_open {
        return panel.child(diff);
    }
    panel.child(diff_with_files_tree(s, diff, t))
}

fn diff_with_files_tree(s: &Desktop, diff: El, t: &Theme) -> El {
    let tree = Tree::new(
        |s: &Desktop| &s.widgets.git_diff_tree,
        |s| &mut s.widgets.git_diff_tree,
        |node| &node.name,
    )
    .row(|node, row| {
        let item = TreeItem::file(node, row);
        node.data.map_or_else(
            || TreeItem::file(node, row),
            |(_, added, removed)| item.changes(added, removed),
        )
    })
    .on_select(|s: &mut Desktop, path, tx| {
        if let Some(i) = s.widgets.git_diff.file_index(path) {
            s.widgets.git_diff.set_collapsed(i, false);
            s.widgets.git_diff.scroll_to_file(i, tx);
        }
    })
    .name("Changed files")
    .build(s, &t.k);
    row()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .child(diff)
        .child(rune::hairline(t.hairline).h_full())
        .child(
            col()
                .w(160.0)
                .min_h(0.0)
                .child(
                    rune_kit::SidebarHeader::new("Files")
                        .build(&t.k)
                        .key("diff-files-title"),
                )
                .child(tree.grow(1.0).min_h(0.0)),
        )
}

fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let split = s.widgets.git_diff.mode() == DiffMode::Split;
    let stats = git
        .diff
        .as_ref()
        .map(|diff| diff.stats)
        .filter(|stats| stats.added > 0 || stats.removed > 0)
        .map(|stats| stat(stats, t));
    row()
        .key("diff-toolbar")
        .h(CHAT_HEADER_HEIGHT)
        .px(S8)
        .gap(S6)
        .w_full()
        .center_y()
        .child(scope(s, t, layer))
        .children(stats)
        .child(rune::spacer())
        .child(
            row()
                .gap(S2)
                .center_y()
                .child(
                    rune_kit::IconButton::new(&ui::COLUMNS2)
                        .help(if split { "Unified diff" } else { "Split diff" })
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Small.kit())
                        .expanded(split)
                        .on_press(|s: &mut Desktop| {
                            let next = if s.widgets.git_diff.mode() == DiffMode::Split {
                                DiffMode::Unified
                            } else {
                                DiffMode::Split
                            };
                            s.widgets.git_diff.set_mode(next);
                        })
                        .build(&t.k),
                )
                .child(
                    rune_kit::IconButton::new(&ui::FOLDER_TREE)
                        .help("Changed files")
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Small.kit())
                        .expanded(s.widgets.git_tree_open)
                        .on_press(|s: &mut Desktop| {
                            s.widgets.git_tree_open = !s.widgets.git_tree_open;
                        })
                        .build(&t.k),
                )
                .child(
                    rune_kit::IconButton::new(&ui::REFRESH_CW)
                        .help("Refresh diff")
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Small.kit())
                        .on_press_tx(|s: &mut Desktop, tx| {
                            super::dispatch(s, GitMsg::RefreshDiff, tx);
                        })
                        .build(&t.k),
                )
                .child(
                    rune_kit::IconButton::new(&ui::CLOSE)
                        .help("Close diff")
                        .variant(IconButtonVariant::GhostMuted)
                        .control_size(Size::Small.kit())
                        .on_press_tx(|s: &mut Desktop, tx| {
                            super::dispatch(s, GitMsg::ToggleDiff, tx);
                        })
                        .build(&t.k),
                ),
        )
}

fn empty_panel(title: &str, detail: Option<&str>, t: &Theme) -> El {
    let mut state = ui::empty_state(title).icon(&ui::FILE_DIFF).compact();
    if let Some(detail) = detail {
        state = state.detail(detail);
    }
    col()
        .grow(1.0)
        .w_full()
        .center()
        .padding(S12)
        .child(state.build(t))
}

fn scope(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let open = git.popup == Some(Popup::Scope);
    let base = default_branch(&s.model).to_owned();
    let label = scope_label(&git.target);
    let trigger = scope_trigger(&label, open, t);
    let mut items = vec![
        scope_item(
            "Working tree",
            "Uncommitted changes",
            git.target == GitDiffTarget::Working,
            GitDiffTarget::Working,
        ),
        scope_item(
            "Staged changes",
            "Ready to commit",
            git.target == GitDiffTarget::Staged,
            GitDiffTarget::Staged,
        ),
        scope_item(
            &format!("Compare to {base}"),
            "Changes since base branch",
            matches!(git.target, GitDiffTarget::Base { .. }),
            GitDiffTarget::Base { revision: base },
        ),
        scope_item(
            "Thread changes",
            "Files edited in this thread",
            git.target == GitDiffTarget::Thread,
            GitDiffTarget::Thread,
        ),
    ];
    match &git.target {
        GitDiffTarget::Commit { revision } => items.push(scope_item(
            &format!("Commit {revision}"),
            "Pinned commit diff",
            true,
            git.target.clone(),
        )),
        GitDiffTarget::Turn { run } => items.push(scope_item(
            &format!("Turn #{run}"),
            "Checkpoint diff",
            true,
            git.target.clone(),
        )),
        _ => {}
    }
    ui::menu_items(Menu::new(SCOPE), items)
        .trigger(trigger)
        .open(open)
        .placement(Placement::BottomStart)
        .min_width(220.0)
        .on_close(super::close_popup)
        .build(&t.overlay, layer)
}

fn scope_label(target: &GitDiffTarget) -> String {
    match target {
        GitDiffTarget::Working => "Working tree".to_owned(),
        GitDiffTarget::Staged => "Staged".to_owned(),
        GitDiffTarget::Base { revision } => format!("vs {revision}"),
        GitDiffTarget::Commit { revision } => {
            let short = &revision[..revision.len().min(7)];
            format!("Commit {short}")
        }
        GitDiffTarget::Turn { run } => format!("Turn #{run}"),
        GitDiffTarget::Thread => "Thread changes".to_owned(),
    }
}

fn scope_trigger(label: &str, open: bool, t: &Theme) -> El {
    row()
        .key(format!("{SCOPE}-trigger"))
        .role("combobox")
        .label("Diff scope")
        .a11y_value(label)
        .expanded(open)
        .h(Size::Small.height())
        .px(S8)
        .gap(S4)
        .center_y()
        .radius(Size::Small.radius())
        .fill(if open {
            t.hover_wash
        } else {
            rune::Color::TRANSPARENT
        })
        .hover_group()
        .hover(t.hover_wash)
        .press(t.pressed_wash)
        .focusable(true)
        .on_press_tx(|s: &mut Desktop, tx| super::toggle_popup(s, Popup::Scope, tx))
        .child(
            text(label)
                .font(t.small_medium)
                .color(t.text)
                .shrink()
                .min_w(0.0)
                .truncate()
                .a11y_hidden(),
        )
        .child(
            icon(&ui::CHEVRON_OPEN, ICON_SMALL)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}

fn scope_item(title: &str, description: &str, selected: bool, target: GitDiffTarget) -> MenuItem {
    MenuItem::new(title)
        .description(description)
        .checked(selected)
        .on_select_tx(move |s: &mut Desktop, tx| {
            super::dispatch(s, GitMsg::Scope(target.clone()), tx);
        })
}

fn run_id(value: &str) -> Option<RunId> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&id| id > 0)
        .map(RunId)
}

pub fn restore_dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let GitDiffTarget::Turn { run } = s.model.git.target else {
        return;
    };
    let input = rune_kit::Field::new()
        .label("Latest completed run ID")
        .control(
            rune_kit::TextInput::new(&s.widgets.git_body)
                .label("Latest completed run ID")
                .placeholder("Run ID")
                .width(None)
                .on_edit(|s: &mut Desktop| &mut s.widgets.git_body)
                .build(&t.k),
        )
        .build(&t.k);
    let footer = ui::dialog_actions([
        rune_kit::Button::new("Cancel")
            .variant(ButtonVariant::Ghost)
            .control_size(Size::Default.kit())
            .on_press(super::close_dialog)
            .build(&t.k),
        rune_kit::Button::new("Restore")
            .variant(ButtonVariant::Danger)
            .control_size(Size::Default.kit())
            .disabled(s.model.git.busy || run_id(&s.widgets.git_body.text).is_none())
            .on_press_tx(move |s: &mut Desktop, tx| {
                if let Some(expected_run) = run_id(&s.widgets.git_body.text) {
                    super::dispatch(
                        s,
                        GitMsg::Request(Request::Restore { run, expected_run }),
                        tx,
                    );
                }
            })
            .build(&t.k),
    ]);
    let description = format!(
        "Restore files to their state before run {run}. The server verifies the checkout against the expected run."
    );
    rune_kit::Dialog::new("restore-checkpoint", "Restore turn checkpoint?")
        .open(true)
        .fit()
        .on_close(super::close_dialog)
        .description(&description)
        .child(ui::dialog_body(
            std::iter::once(input).chain(super::actions::error(s, t)),
            footer,
        ))
        .build(&t.overlay, layer);
}
