//! Turn-level `ChangedFilesTree` card with compacted folders
//! (`rune-kit::FileTree`), file icons, per-file `+N -M` diff stats,
//! expand/collapse-all, and `"Open diff"` actions.

use std::f32::consts::FRAC_PI_2;

use rune::{El, Tx, col, icon, row, text};
use rune_kit::{FileNode, FileTree, TreeModel, file_icon};

use crate::{
    app::Desktop,
    model::{ChangedFile, TurnView},
    motion,
    theme::{
        CARD_INSET, HAIRLINE, ICON_MD, ICON_SMALL, RADIUS_CARD, RADIUS_SM, S2, S6, S8, S12, S14,
        S24, Theme,
    },
    ui::{self, Card},
    update::{Msg, TimelineMsg},
};

/// Renders the turn-level `ChangedFilesTree` card below assistant prose when
/// the turn edited or wrote files.
pub fn changed_files_card(s: &Desktop, turn: &TurnView<'_>, t: &Theme) -> Option<El> {
    if turn.changed_files.is_empty() {
        return None;
    }
    let tree = FileTree::new()
        .compact_folders(true)
        .dirs_first(true)
        .key(format!("{}-files", turn.key))
        .build(
            turn.changed_files
                .iter()
                .map(|file| (file.path.as_str(), file.clone())),
        );
    let prefix = format!("{}-cf", turn.key);
    let mut folder_keys = Vec::new();
    collect_folder_keys(&tree, None, &prefix, &mut folder_keys);

    let m = &s.model;
    let all_open = folder_keys.iter().all(|key| m.is_fold_open(key, true));
    let header = changed_files_header(turn, folder_keys, all_open, t);
    let mut rows = Vec::new();
    render_branch(s, &tree, None, 0, &prefix, &mut rows, t);

    let mut card = Card::Surface
        .frame(t)
        .key("card")
        .role("region")
        .label(format!("Changed files ({})", turn.changed_files.len()))
        .clip_rounded(RADIUS_CARD)
        .child(header)
        .child(ui::rule(t))
        .child(
            col()
                .key("tree-rows")
                .w_full()
                .px(S6)
                .py(S6)
                .gap(S2)
                .children(rows),
        );
    if !t.k.dark {
        card = card.border(HAIRLINE, t.border_control);
    }
    Some(
        col()
            .key(format!("{}-changed-files", turn.key))
            .w_full()
            .pt(S8)
            .child(card),
    )
}

fn changed_files_header(
    turn: &TurnView<'_>,
    folder_keys: Vec<String>,
    all_open: bool,
    t: &Theme,
) -> El {
    let (total_add, total_del) = turn.total_diff_stats();
    let count = turn.changed_files.len();
    let title = format!(
        "{count} changed {}",
        crate::model::plural(count, "file", "files")
    );
    let mut header = row()
        .key("header")
        .w_full()
        .px(CARD_INSET)
        .py(S8)
        .gap(S8)
        .center_y()
        .child(
            text(title)
                .font(t.small_medium)
                .color(t.text_strong)
                .truncate()
                .shrink(),
        )
        .child(ui::diff_stat(total_add, total_del, t))
        .child(rune::el().grow(1.0).min_w(0.0).a11y_hidden());

    if !folder_keys.is_empty() {
        let next_expand = !all_open;
        header = header.child(
            rune_kit::Button::new(if all_open {
                "Collapse all"
            } else {
                "Expand all"
            })
            .variant(rune_kit::ButtonVariant::GhostMuted)
            .control_size(crate::theme::Size::Micro.kit())
            .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(
                    Msg::Timeline(TimelineMsg::ToggleChangedFolders {
                        keys: folder_keys.clone(),
                        expand: next_expand,
                    }),
                    tx,
                );
            })
            .build(&t.k)
            .key("toggle-all"),
        );
    }

    header.child(
        rune_kit::Button::new("Open diff")
            .variant(rune_kit::ButtonVariant::GhostMuted)
            .control_size(crate::theme::Size::Micro.kit())
            .icon(&ui::COLUMNS2)
            .on_press_tx(|s: &mut Desktop, tx: &mut Tx| {
                s.dispatch(Msg::Timeline(TimelineMsg::OpenDiff { path: None }), tx);
            })
            .build(&t.k)
            .key("open-all-diffs"),
    )
}

fn collect_folder_keys(
    tree: &TreeModel<FileNode<ChangedFile>>,
    parent: Option<&str>,
    prefix: &str,
    out: &mut Vec<String>,
) {
    for id in tree.children(parent) {
        if let Some(node) = tree.get(id)
            && node.dir
        {
            out.push(format!("{prefix}-dir-{}", node.path));
            collect_folder_keys(tree, Some(id), prefix, out);
        }
    }
}

fn render_branch(
    s: &Desktop,
    tree: &TreeModel<FileNode<ChangedFile>>,
    parent: Option<&str>,
    depth: u32,
    prefix: &str,
    out: &mut Vec<El>,
    t: &Theme,
) {
    let m = &s.model;
    #[expect(
        clippy::cast_precision_loss,
        reason = "tree depth stays well under 64 levels"
    )]
    let indent = (depth as f32).mul_add(S14, S6);

    for id in tree.children(parent) {
        let Some(node) = tree.get(id) else {
            continue;
        };
        if node.dir {
            let fold_key = format!("{prefix}-dir-{}", node.path);
            let open = m.is_fold_open(&fold_key, true);
            out.push(folder_row(node, fold_key, open, indent, t));
            if open {
                render_branch(s, tree, Some(id), depth + 1, prefix, out, t);
            }
        } else if let Some(file) = &node.data {
            let selected = m.timeline.selected_diff_file.as_deref() == Some(node.path.as_str());
            out.push(file_row(node, file, prefix, selected, indent, t));
        }
    }
}

fn folder_row(
    node: &FileNode<ChangedFile>,
    fold_key: String,
    open: bool,
    indent: f32,
    t: &Theme,
) -> El {
    let row_key = format!("{fold_key}-row");
    let anchor = row_key.clone();
    row()
        .key(row_key)
        .role("button")
        .label(format!("Folder {}", node.name))
        .expanded(open)
        .w_full()
        .min_h(S24)
        .pl(indent)
        .pr(S8)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::ToggleFold {
                    key: fold_key.clone(),
                    default_open: true,
                    anchor: Some(anchor.clone()),
                }),
                tx,
            );
        })
        .child(
            motion::chevron(&ui::CHEVRON, ICON_SMALL, open, FRAC_PI_2)
                .key("chevron")
                .color(t.faint)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            icon(if open { &ui::FOLDER_OPEN } else { &ui::FOLDER }, ICON_MD)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(&node.name)
                .font(t.mono_small)
                .color(t.muted)
                .group_color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate(),
        )
}

fn file_row(
    node: &FileNode<ChangedFile>,
    file: &ChangedFile,
    prefix: &str,
    selected: bool,
    indent: f32,
    t: &Theme,
) -> El {
    let open_path = node.path.clone();
    let mut row_el = row()
        .key(format!("{prefix}-file-{}", node.path))
        .role("button")
        .label(format!("Open diff for {}", node.path))
        .w_full()
        .min_h(S24)
        .pl(indent + S12)
        .pr(S8)
        .py(S2)
        .gap(S6)
        .center_y()
        .radius(RADIUS_SM)
        .hover(t.hover_wash)
        .hover_group()
        .focusable(true)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .on_press_tx(move |s: &mut Desktop, tx: &mut Tx| {
            s.dispatch(
                Msg::Timeline(TimelineMsg::OpenDiff {
                    path: Some(open_path.clone()),
                }),
                tx,
            );
        })
        .child(
            icon(file_icon(&node.name), ICON_MD)
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(&node.name)
                .font(t.mono_small)
                .color(t.text)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate()
                .help(&node.path),
        )
        .child(ui::diff_stat(file.added, file.removed, t));
    if selected {
        row_el = row_el.fill(t.selected_subtle);
    }
    row_el
}
