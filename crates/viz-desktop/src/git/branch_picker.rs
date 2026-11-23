//! Paged checkout ref search and worktree base selection.

use rune::{El, ScrollSpec, col, text};
use rune_kit::Placement;

use crate::{
    app::Desktop,
    model::git::{CARD_WIDTH, Popup},
    theme::Theme,
    ui,
    update::GitMsg,
};

const ANCHOR: &str = "git-branch-trigger";

pub fn view(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) -> El {
    let git = &s.model.git;
    let current = git
        .status
        .as_ref()
        .and_then(|status| status.branch.as_deref());
    let open = git.popup == Some(Popup::Branch);
    let trigger = ui::list_row(ANCHOR, current.unwrap_or("Detached HEAD"))
        .leading_icon(&ui::GIT_BRANCH)
        .detail("Branch")
        .disabled(!s.model.connected() || s.model.busy() || git.busy)
        .on_press_tx(|s: &mut Desktop, tx| {
            s.widgets.git_query = rune::TextEdit::new("");
            super::toggle_popup(s, Popup::Branch, tx);
            tx.focus_when_shown("git-branch-search", false);
        })
        .build(t);
    if open {
        overlays.push(
            ui::anchored_popover(
                "git-branches",
                ANCHOR,
                Placement::BottomEnd,
                CARD_WIDTH,
                contents(s, current, t),
                t,
            )
            .modal(true, super::close_popup),
        );
    }
    trigger
}

fn contents(s: &Desktop, current: Option<&str>, t: &Theme) -> El {
    let git = &s.model.git;
    let active = git
        .branch_highlight
        .and_then(|index| git.branches.as_ref()?.branches.get(index))
        .map(|branch| format!("branch-{}", branch.name));
    let search = ui::search_input(&s.widgets.git_query, "Search refs…")
        .input_key("git-branch-search")
        .style(ui::SearchStyle::Plain)
        .active_descendant(active)
        .on_edit(|s: &mut Desktop| &mut s.widgets.git_query)
        .on_key(on_key)
        .on_change(|s: &mut Desktop, tx| {
            super::dispatch(s, GitMsg::Search(s.widgets.git_query.text.clone()), tx);
        })
        .build(t);
    let rows = results(s, current, t);
    let list = if rows.is_empty() {
        let label = if git.branches.is_some() {
            "No refs found"
        } else {
            "Loading refs…"
        };
        text(git.error.as_deref().unwrap_or(label))
            .font(t.small)
            .color(t.muted)
            .padding(8.0)
    } else {
        col()
            .key("git-branch-scroll")
            .max_h(224.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_anchor()
            .child(
                col()
                    .key("git-branch-list")
                    .growth(rune::GrowthSpec::default())
                    .virtualize(28.0)
                    .on_visible_rows(|s: &mut Desktop, visible| {
                        let Some(page) = &s.model.git.branches else {
                            return false;
                        };
                        if s.model.git.popup != Some(Popup::Branch)
                            || !s.model.connected()
                            || page.next.is_none()
                            || visible.end < page.branches.len().div_ceil(2)
                        {
                            return false;
                        }
                        s.dispatch_local(crate::update::Msg::Git(GitMsg::MoreBranches));
                        true
                    })
                    .children(rows),
            )
    };
    col()
        .w_full()
        .gap(4.0)
        .padding(6.0)
        .child(search)
        .child(ui::rule(t))
        .child(list)
        .children(git.new_worktree.then(|| {
            ui::switch(git.remote_base)
                .label("Start from remote")
                .description("Use the tracked remote branch or the matching branch on origin")
                .on_toggle_tx(|s: &mut Desktop, tx| super::dispatch(s, GitMsg::RemoteBase, tx))
                .build(t)
        }))
}

fn results(s: &Desktop, current: Option<&str>, t: &Theme) -> Vec<El> {
    let git = &s.model.git;
    let mut rows = Vec::new();
    if let Some(page) = &git.branches {
        rows.extend(page.branches.iter().enumerate().map(|(index, branch)| {
            let tag = if current == Some(&branch.name) {
                "current"
            } else if branch.checked_out_elsewhere {
                "worktree"
            } else if branch.remote {
                "remote"
            } else if page.default.as_deref() == Some(&branch.name) {
                "default"
            } else {
                ""
            };
            let name = branch.name.clone();
            let upstream = branch.upstream.clone();
            ui::list_row(format!("branch-{name}"), &name)
                .density(ui::RowDensity::Compact)
                .detail(tag)
                .highlighted(git.branch_highlight == Some(index))
                .selected(tag == "current")
                .disabled(tag == "current" && !git.new_worktree)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    super::dispatch(
                        s,
                        GitMsg::ChooseBranch {
                            name: name.clone(),
                            upstream: upstream.clone(),
                            create: false,
                        },
                        tx,
                    );
                })
                .build(t)
        }));
        let name = git.query.split_whitespace().collect::<Vec<_>>().join("-");
        if !name.is_empty() && !page.branches.iter().any(|branch| branch.name == name) {
            rows.push(
                ui::list_row("create-git-branch", format!("Create branch {name}"))
                    .leading_icon(&ui::GIT_BRANCH_PLUS)
                    .on_press_tx(move |s: &mut Desktop, tx| {
                        super::dispatch(
                            s,
                            GitMsg::ChooseBranch {
                                name: name.clone(),
                                upstream: None,
                                create: true,
                            },
                            tx,
                        );
                    })
                    .build(t),
            );
        }
        if page.next.is_some() {
            rows.push(
                ui::list_row("git-branch-next", "Load more branches")
                    .on_press_tx(|s: &mut Desktop, tx| super::dispatch(s, GitMsg::MoreBranches, tx))
                    .build(t),
            );
        }
    }
    rows
}

fn on_key(s: &mut Desktop, event: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    use rune::Key;
    if event.mods.ctrl
        || event.mods.cmd
        || event.mods.alt
        || event.mods.shift
        || event.mods.logo
        || s.widgets.git_query.preedit().is_some()
    {
        return false;
    }
    let Some(page) = &s.model.git.branches else {
        return false;
    };
    let active = s.model.git.branch_highlight;
    let count = page.branches.len();
    if count == 0 {
        return false;
    }
    let index = match event.key {
        Key::Down => active.map_or(0, |i| (i + 1).min(count - 1)),
        Key::Up => active.map_or(count - 1, |i| i.saturating_sub(1)),
        Key::Enter if !tx.key_repeat() => {
            let Some(branch) = active.and_then(|index| page.branches.get(index)) else {
                return false;
            };
            let current = s
                .model
                .git
                .status
                .as_ref()
                .and_then(|status| status.branch.as_deref());
            if s.model.git.new_worktree || current != Some(&branch.name) {
                let msg = GitMsg::ChooseBranch {
                    name: branch.name.clone(),
                    upstream: branch.upstream.clone(),
                    create: false,
                };
                super::dispatch(s, msg, tx);
            }
            return true;
        }
        _ => return false,
    };
    let key = format!("branch-{}", page.branches[index].name);
    super::dispatch(s, GitMsg::HighlightBranch(index), tx);
    tx.scroll_into_view(key);
    true
}
