//! Searchable branch / base-ref combobox for the Home bottom Context Strip,
//! reusing Track G's cached project checkouts.

use rune::{El, Key, KeyEvent, Tx, icon, text};
use rune_kit::{Placement, scrim};
use viz_client::protocol::{GitBranch, Project, WorktreeTarget};

use super::context_strip::{close_picker, strip_pill, worktree_branch_name};
use crate::{
    app::Desktop,
    model::{NewThread, draft_branch_options},
    motion,
    theme::{ICON_SMALL, S6, Size, Theme},
    ui::{self, RowDensity, SearchStyle},
    update::{ComposerMsg, DRAFT_BRANCH_PICKER, DRAFT_BRANCH_SEARCH, Msg, NavKey, SelectionMsg},
};

const BRANCH_POPOVER_WIDTH: f32 = 280.0;

/// Renders the Branch / Base-ref selector pill in the Home Context Strip and
/// mounts its searchable combobox popover when open.
pub(super) fn selector(
    s: &Desktop,
    draft: Option<&NewThread>,
    project: &Project,
    compact: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    const TRIGGER: &str = "draft-branch-trigger";
    let m = &s.model;
    let sending = draft.is_some_and(|d| d.sending);
    let open = m.composer.open_picker.as_deref() == Some(DRAFT_BRANCH_PICKER) && !sending;
    let checkouts = m.checkouts(&project.path);
    let is_new_worktree =
        draft.is_some_and(|d| matches!(d.worktree, Some(WorktreeTarget::New { .. })));
    let remote_base = draft.is_some_and(|d| d.remote_base);
    let current_ref = match draft.and_then(|d| d.worktree.as_ref()) {
        Some(WorktreeTarget::New { base, .. }) => base.as_deref().or(project.branch.as_deref()),
        Some(WorktreeTarget::Existing { path }) => {
            let name = worktree_branch_name(checkouts, path);
            return strip_pill(
                TRIGGER,
                &format!("Branch: {name}"),
                &ui::GIT_BRANCH,
                (!compact).then_some(name.as_str()),
                false,
                true,
                false,
                t,
            );
        }
        None | Some(WorktreeTarget::Local) => project.branch.as_deref(),
    };
    let label = match (is_new_worktree, current_ref, remote_base) {
        (true, Some(base), true) => format!("From {base} (remote)"),
        (true, Some(base), false) => format!("From {base}"),
        (false, Some(branch), _) => branch.to_owned(),
        (_, None, _) => "Select ref".to_owned(),
    };
    let path = project.path.clone();
    let trigger = strip_pill(
        TRIGGER,
        &format!("Branch: {label}"),
        &ui::GIT_BRANCH,
        (!compact).then_some(label.as_str()),
        open,
        sending,
        true,
        t,
    )
    .on_press_tx(move |s: &mut Desktop, tx| {
        s.dispatch(
            Msg::Composer(ComposerMsg::ToggleDraftBranchPicker(path.clone())),
            tx,
        );
    });
    if open {
        popover(
            s,
            project,
            current_ref,
            is_new_worktree,
            remote_base,
            TRIGGER,
            t,
            layer,
        );
    }
    trigger
}

#[expect(
    clippy::too_many_arguments,
    reason = "anchored draft branch combobox receives trigger key and resolved draft state"
)]
fn popover(
    s: &Desktop,
    project: &Project,
    current: Option<&str>,
    is_new_worktree: bool,
    remote_base: bool,
    anchor: &str,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let branches = draft_branch_options(&s.model, project);
    let active_idx = s
        .model
        .composer
        .draft_branch
        .highlighted
        .min(branches.len().saturating_sub(1));
    let active_key = branches
        .get(active_idx)
        .map(|branch| format!("draft-branch-{}", branch.name));
    let search = ui::search_input(&s.widgets.draft_branch_query, "Search refs…")
        .label("Search branches")
        .input_key(DRAFT_BRANCH_SEARCH)
        .style(SearchStyle::Plain)
        .size(Size::Compact)
        .active_descendant(active_key)
        .on_edit(|s: &mut Desktop| &mut s.widgets.draft_branch_query)
        .on_change(|s: &mut Desktop, tx| {
            let query = s.widgets.draft_branch_query.text.clone();
            s.dispatch(
                Msg::Composer(ComposerMsg::DraftBranchQueryChanged(query)),
                tx,
            );
        })
        .on_key(on_branch_key)
        .build(t);
    let rows = branches
        .iter()
        .enumerate()
        .map(|(index, branch)| branch_row(branch, project, current, index == active_idx, t));
    let empty_text = if s.model.checkouts(&project.path).is_some() {
        "No matching refs."
    } else {
        "Loading refs…"
    };
    let mut shell = ui::combobox_shell("draft-branch-combobox")
        .width(BRANCH_POPOVER_WIDTH)
        .max_height(224.0)
        .search(search)
        .items(rows)
        .empty(text(empty_text).font(t.small).color(t.faint).role("status"));
    if is_new_worktree {
        shell = shell.footer(
            rune_kit::Switch::new(remote_base)
                .label("Start from remote")
                .on_toggle_tx(|s: &mut Desktop, tx| {
                    s.dispatch(Msg::Selection(SelectionMsg::ToggleDraftRemoteBase), tx);
                })
                .build(&t.k),
        );
    }
    let popover = shell
        .build(t)
        .role("dialog")
        .label("Choose branch")
        .focus_return(anchor)
        .modal(true, close_picker);
    layer.push(scrim("draft-branch-scrim", close_picker));
    layer.push(motion::anchored_popover(
        popover,
        anchor,
        Placement::TopStart,
        S6,
    ));
}

fn branch_row(
    branch: &GitBranch,
    project: &Project,
    current: Option<&str>,
    highlighted: bool,
    t: &Theme,
) -> El {
    let selected = current == Some(branch.name.as_str());
    let tag = if selected {
        "current"
    } else if branch.checked_out_elsewhere {
        "worktree"
    } else if branch.remote {
        "remote"
    } else if project.branch.as_deref() == Some(branch.name.as_str()) {
        "default"
    } else {
        ""
    };
    let name = branch.name.clone();
    let upstream = branch.upstream.clone();
    let mut row = ui::list_row(format!("draft-branch-{}", branch.name), &branch.name)
        .density(RowDensity::Compact)
        .selected(selected)
        .highlighted(highlighted)
        .focus_ring_for(DRAFT_BRANCH_SEARCH)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Selection(SelectionMsg::ChooseDraftBranch {
                    name: name.clone(),
                    upstream: upstream.clone(),
                }),
                tx,
            );
        });
    if !tag.is_empty() {
        row = row.detail(tag);
    }
    if selected {
        row = row.trailing(
            icon(&ui::CHECK, ICON_SMALL)
                .color(t.accent_text)
                .a11y_hidden(),
        );
    }
    row.build(t)
}

fn on_branch_key(s: &mut Desktop, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.shift || ev.mods.logo {
        return false;
    }
    if s.widgets.draft_branch_query.preedit().is_some() {
        return false;
    }
    let key = match ev.key {
        Key::Down => NavKey::Down,
        Key::Up => NavKey::Up,
        Key::Enter if !tx.key_repeat() => NavKey::Enter,
        Key::Enter => return true,
        Key::Escape => NavKey::Escape,
        _ => return false,
    };
    s.dispatch(Msg::Composer(ComposerMsg::DraftBranchKey(key)), tx);
    true
}
