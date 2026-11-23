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
    let choices = choices(s, current);
    let active = git
        .branch_highlight
        .as_ref()
        .filter(|key| choices.iter().any(|choice| choice.key == **key))
        .cloned();
    let search = rune_kit::TextInput::new(&s.widgets.git_query)
        .placeholder("Search refs…")
        .leading_icon(&ui::SEARCH)
        .width(None)
        .plain()
        .active_descendant(active)
        .on_edit(|s: &mut Desktop| &mut s.widgets.git_query)
        .on_key(on_key)
        .on_change(|s: &mut Desktop, tx| {
            super::dispatch(s, GitMsg::Search(s.widgets.git_query.text.clone()), tx);
        })
        .build(&t.k)
        .key("git-branch-search");
    let rows: Vec<_> = choices
        .into_iter()
        .map(|choice| {
            let mut row = ui::list_row(&choice.key, &choice.title)
                .density(ui::RowDensity::Compact)
                .detail(choice.tag)
                .highlighted(git.branch_highlight.as_ref() == Some(&choice.key))
                .selected(choice.tag == "current")
                .disabled(choice.disabled);
            if choice.key == "create-git-branch" {
                row = row.leading_icon(&ui::GIT_BRANCH_PLUS);
            }
            row.on_press_tx(move |s: &mut Desktop, tx| {
                super::dispatch(s, choice.message.clone(), tx);
            })
            .build(t)
        })
        .collect();
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
            rune_kit::Switch::new(git.remote_base)
                .label("Start from remote")
                .description("Use the tracked remote branch or the matching branch on origin")
                .on_toggle_tx(|s: &mut Desktop, tx| super::dispatch(s, GitMsg::RemoteBase, tx))
                .build(&t.k)
        }))
}

struct BranchChoice {
    key: String,
    title: String,
    tag: &'static str,
    disabled: bool,
    message: GitMsg,
}

fn choices(s: &Desktop, current: Option<&str>) -> Vec<BranchChoice> {
    let git = &s.model.git;
    let Some(page) = &git.branches else {
        return Vec::new();
    };
    let mut rows: Vec<_> = page
        .branches
        .iter()
        .map(|branch| {
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
            BranchChoice {
                key: format!("branch-{}", branch.name),
                title: branch.name.clone(),
                tag,
                disabled: tag == "current" && !git.new_worktree,
                message: GitMsg::ChooseBranch {
                    name: branch.name.clone(),
                    upstream: branch.upstream.clone(),
                    create: false,
                },
            }
        })
        .collect();
    let name = git.query.split_whitespace().collect::<Vec<_>>().join("-");
    if !name.is_empty() && !page.branches.iter().any(|branch| branch.name == name) {
        rows.push(BranchChoice {
            key: "create-git-branch".into(),
            title: format!("Create branch {name}"),
            tag: "",
            disabled: false,
            message: GitMsg::ChooseBranch {
                name,
                upstream: None,
                create: true,
            },
        });
    }
    if page.next.is_some() {
        rows.push(BranchChoice {
            key: "git-branch-next".into(),
            title: "Load more branches".into(),
            tag: "",
            disabled: false,
            message: GitMsg::MoreBranches,
        });
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
    let git = &s.model.git;
    let current = git
        .status
        .as_ref()
        .and_then(|status| status.branch.as_deref());
    let choices: Vec<_> = choices(s, current)
        .into_iter()
        .filter(|choice| !choice.disabled)
        .collect();
    let active = choices
        .iter()
        .position(|choice| git.branch_highlight.as_ref() == Some(&choice.key));
    let count = choices.len();
    if count == 0 {
        return false;
    }
    let index = match event.key {
        Key::Down => active.map_or(0, |i| (i + 1).min(count - 1)),
        Key::Up => active.map_or(count - 1, |i| i.saturating_sub(1)),
        Key::Enter if !tx.key_repeat() => {
            let Some(index) = active else {
                return false;
            };
            super::dispatch(s, choices[index].message.clone(), tx);
            return true;
        }
        _ => return false,
    };
    let key = choices[index].key.clone();
    super::dispatch(s, GitMsg::HighlightBranch(key.clone()), tx);
    tx.scroll_into_view(key);
    true
}
