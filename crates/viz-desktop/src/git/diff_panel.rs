//! Docked structured diff viewer and checkpoint controls.

use std::fmt::Write as _;

use rune::{El, col, row, text};
use rune_kit::{
    ButtonVariant, FileTree, IconButtonVariant, Placement, Popover, RadioGroup, RadioItem, Tree,
    TreeItem,
    diff::{self, DiffMode, DiffView, FileDiff, FileStatus},
};
use viz_client::protocol::{GitDiffFile, GitDiffTarget, GitFileKind, GitLineStats, RunId};

use crate::{
    app::Desktop,
    effects::git::Request,
    model::git::{Dialog, Popup, default_branch},
    theme::{Size, Theme},
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
    let mut patch = String::from("diff --git a/file b/file\n--- a/file\n+++ b/file\n");
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
            rune_kit::Button::new("Restore checkpoint…")
                .variant(ButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .disabled(s.model.busy())
                .on_press_tx(|s: &mut Desktop, tx| {
                    s.widgets.git_body = rune::TextEdit::new("");
                    super::dispatch(s, GitMsg::Dialog(Some(Dialog::Restore)), tx);
                })
                .build(&t.k),
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
                .padding(8.0),
        );
    }
    if let Some(error) = &git.diff_error {
        return panel.child(message(error, t));
    }
    if s.widgets.git_diff.files().is_empty() {
        return panel.child(message(
            if git.diff.is_none() {
                "Loading diff…"
            } else {
                "No changes in this scope"
            },
            t,
        ));
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
    panel.child(
        row()
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .child(diff)
            .child(rune::hairline(t.hairline).h_full())
            .child(
                col()
                    .w(144.0)
                    .min_h(0.0)
                    .child(
                        rune_kit::SidebarHeader::new("Files")
                            .build(&t.k)
                            .key("diff-files-title"),
                    )
                    .child(tree.grow(1.0).min_h(0.0)),
            ),
    )
}

fn toolbar(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let selected = usize::from(s.widgets.git_diff.mode() == DiffMode::Split);
    row()
        .key("diff-toolbar")
        .h(40.0)
        .px(8.0)
        .gap(4.0)
        .w_full()
        .center_y()
        .child(scope(s, t, layer))
        .child(el_spacer())
        .children(git.diff.as_ref().map(|d| stat(d.stats, t)))
        .child(
            rune_kit::SegmentedControl::new(&["Unified", "Split"], Some(selected))
                .fit()
                .label("Diff layout")
                .on_pick(|s: &mut Desktop, index, _| {
                    s.widgets.git_diff.set_mode(if index == 0 {
                        DiffMode::Unified
                    } else {
                        DiffMode::Split
                    });
                })
                .build(&t.k),
        )
        .child(
            rune_kit::IconButton::new(&ui::REFRESH_CW)
                .help("Refresh diff")
                .variant(IconButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .on_press_tx(|s: &mut Desktop, tx| super::dispatch(s, GitMsg::RefreshDiff, tx))
                .build(&t.k),
        )
        .child(
            rune_kit::IconButton::new(&ui::FOLDER_TREE)
                .help("Toggle changed files")
                .variant(IconButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .on_press(|s: &mut Desktop| s.widgets.git_tree_open = !s.widgets.git_tree_open)
                .build(&t.k),
        )
        .child(
            rune_kit::IconButton::new(&ui::CLOSE)
                .help("Close diff")
                .variant(IconButtonVariant::Ghost)
                .control_size(Size::Small.kit())
                .on_press_tx(|s: &mut Desktop, tx| super::dispatch(s, GitMsg::ToggleDiff, tx))
                .build(&t.k),
        )
}

fn el_spacer() -> El {
    rune::el().grow(1.0)
}

fn message(label: &str, t: &Theme) -> El {
    col()
        .grow(1.0)
        .w_full()
        .center()
        .padding(12.0)
        .child(text(label).font(t.small).color(t.muted).wrap_anywhere())
}

fn scope(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let open = git.popup == Some(Popup::Scope);
    let label = match git.target {
        GitDiffTarget::Working => "Working",
        GitDiffTarget::Staged => "Staged",
        GitDiffTarget::Base { .. } => "vs Base",
        GitDiffTarget::Commit { .. } => "Commit",
        GitDiffTarget::Turn { .. } => "Turn",
        GitDiffTarget::Thread => "Thread",
    };
    let base = default_branch(&s.model).to_owned();
    let scopes = [
        ("Working", GitDiffTarget::Working),
        ("Staged", GitDiffTarget::Staged),
        ("vs Base", GitDiffTarget::Base { revision: base }),
        ("Thread changes", GitDiffTarget::Thread),
    ]
    .into_iter()
    .fold(RadioGroup::new(), |group, (label, target)| {
        group.item(RadioItem::new(label, target == git.target).on_select_tx(
            move |s: &mut Desktop, tx| {
                super::dispatch(s, GitMsg::Scope(target.clone()), tx);
            },
        ))
    });
    let trigger = rune_kit::Button::new(label)
        .variant(ButtonVariant::Outline)
        .control_size(Size::Small.kit())
        .on_press_tx(|s: &mut Desktop, tx| super::toggle_popup(s, Popup::Scope, tx))
        .build(&t.k);
    Popover::new(SCOPE)
        .trigger(trigger)
        .title("Diff scope")
        .open(open)
        .placement(Placement::BottomStart)
        .on_close(super::close_popup)
        .child(
            col()
                .w_full()
                .gap(8.0)
                .child(scopes.build(&t.overlay))
                .child(input_scope(
                    &s.widgets.git_revision,
                    "Commit revision",
                    false,
                    t,
                ))
                .child(input_scope(&s.widgets.git_run, "Turn run ID", true, t)),
        )
        .build(&t.overlay, layer)
}

fn input_scope(edit: &rune::TextEdit, label: &str, turn: bool, t: &Theme) -> El {
    rune_kit::TextInput::new(edit)
        .label(label)
        .placeholder(label)
        .width(None)
        .on_edit(move |s: &mut Desktop| {
            if turn {
                &mut s.widgets.git_run
            } else {
                &mut s.widgets.git_revision
            }
        })
        .on_submit(move |s: &mut Desktop, tx| {
            let target = if turn {
                let Some(run) = run_id(&s.widgets.git_run.text) else {
                    return;
                };
                GitDiffTarget::Turn { run }
            } else {
                let revision = s.widgets.git_revision.text.trim().to_owned();
                if revision.is_empty() {
                    return;
                }
                GitDiffTarget::Commit { revision }
            };
            super::dispatch(s, GitMsg::Scope(target), tx);
        })
        .build(&t.k)
        .key(label)
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
