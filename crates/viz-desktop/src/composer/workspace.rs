use rune::El;
use rune_kit::MenuItem;
use viz_client::protocol::{Branches, ManagedWorktree, Workspace};

use super::settings::{GAP, is_open, picker, picker_width};
use crate::{
    app::{Desktop, NewChat},
    theme::{S8, Size, Theme, WORKSPACE_MENU_WIDTH},
    ui,
};

const WORKSPACE: &str = "workspace";
const BASE: &str = "workspace-base";
const HARNESS: &str = "harness";
const LOCAL: &str = "Current checkout";
const NEW: &str = "New worktree";

pub(super) fn locked(s: &Desktop) -> Option<(&str, &str)> {
    if let Some((path, branch)) = s.worktree() {
        return Some((branch.unwrap_or("Detached worktree"), path));
    }
    let state = s.conversation.as_ref()?;
    state.git.as_ref()?;
    let path = state.project.as_ref().and(state.cwd.as_deref())?;
    Some((LOCAL, path))
}

pub(super) fn toolbar(
    s: &Desktop,
    chat: &NewChat,
    mut bar: El,
    t: &Theme,
    layer: &mut Vec<El>,
) -> (El, f32) {
    if chat.sending {
        return (
            bar.gap(S8)
                .child(ui::lead(ui::spinner(t), Size::Default.height()))
                .child(ui::section("Opening session", t)),
            0.0,
        );
    }
    let mut width = 0.0;
    if let Some(branches) = &chat.branches {
        let (workspace, size) = workspace(s, chat, t, layer);
        bar = bar.child(workspace);
        width += size + GAP;
        if let Workspace::Worktree { base } = &chat.workspace {
            let (base, size) = base_picker(s, branches, base.as_deref(), t, layer);
            bar = bar.child(base);
            width += size + GAP;
        }
    }
    if s.harnesses.is_empty() {
        return (bar.child(ui::section("No harness configured", t)), width);
    }
    if s.harnesses.len() > 1 {
        let (harness, size) = harness(s, chat, t, layer);
        bar = bar.child(harness);
        width += size + GAP;
    }
    (bar, width)
}

fn previous(worktree: &ManagedWorktree) -> String {
    worktree.branch.as_deref().map_or_else(
        || "Previous worktree (detached)".into(),
        |branch| format!("Previous worktree ({branch})"),
    )
}

fn workspace(s: &Desktop, chat: &NewChat, t: &Theme, layer: &mut Vec<El>) -> (El, f32) {
    let label = match &chat.workspace {
        Workspace::Local => LOCAL.into(),
        Workspace::Worktree { .. } => NEW.into(),
        Workspace::Existing { path } => chat
            .worktrees
            .iter()
            .find(|entry| entry.path == *path)
            .map_or_else(|| "Previous worktree".into(), previous),
    };
    let mut menu =
        picker(s, WORKSPACE, "Workspace", Some(&label), t).min_width(WORKSPACE_MENU_WIDTH);
    if is_open(s, WORKSPACE) {
        menu = menu
            .group_label("Workspace")
            .item(choice(
                LOCAL,
                chat.workspace == Workspace::Local,
                Workspace::Local,
            ))
            .item(choice(
                NEW,
                matches!(chat.workspace, Workspace::Worktree { .. }),
                Workspace::Worktree { base: None },
            ));
        if !chat.worktrees.is_empty() {
            menu = menu.separator();
        }
        for worktree in &chat.worktrees {
            let value = Workspace::Existing {
                path: worktree.path.clone(),
            };
            let mut item = choice(&previous(worktree), chat.workspace == value, value);
            if worktree.branch.is_none() {
                item = item.description(&worktree.path);
            }
            menu = menu.item(item);
        }
    }
    (
        menu.build(&t.k, layer).min_w(0.0).shrink(),
        picker_width(&label, t),
    )
}

fn base_picker(
    s: &Desktop,
    branches: &Branches,
    base: Option<&str>,
    t: &Theme,
    layer: &mut Vec<El>,
) -> (El, f32) {
    let label = base_label(base.or(branches.default_base.as_deref()));
    let mut menu = picker(s, BASE, "Worktree base", Some(&label), t);
    if is_open(s, BASE) {
        menu = menu.group_label("Worktree base").item(
            choice(
                &base_label(branches.default_base.as_deref()),
                base.is_none(),
                Workspace::Worktree { base: None },
            )
            .badge("Default"),
        );
        let choices = branches
            .branches
            .iter()
            .filter(|branch| branches.default_base.as_ref() != Some(*branch))
            .map(|branch| {
                choice(
                    &base_label(Some(branch)),
                    base == Some(branch.as_str()),
                    Workspace::Worktree {
                        base: Some(branch.clone()),
                    },
                )
            });
        menu = ui::menu_items(menu, choices);
    }
    (
        menu.build(&t.k, layer).min_w(0.0).shrink(),
        picker_width(&label, t),
    )
}

fn base_label(branch: Option<&str>) -> String {
    branch.map_or_else(|| "Default base".into(), |branch| format!("From {branch}"))
}

fn choice(label: &str, selected: bool, value: Workspace) -> MenuItem {
    MenuItem::new(label)
        .checked(selected)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.open_picker = None;
            tx.focus("draft", false);
            if !selected && let Some(chat) = &mut s.new_chat {
                chat.workspace = value.clone();
            }
        })
}

fn harness(s: &Desktop, chat: &NewChat, t: &Theme, layer: &mut Vec<El>) -> (El, f32) {
    let label = "Choose harness";
    let name = chat
        .active_harness(&s.harnesses)
        .map(|runner| runner.name.as_str());
    let mut menu = picker(s, HARNESS, label, name, t);
    if is_open(s, HARNESS) {
        menu = ui::menu_items(
            menu,
            s.harnesses.iter().map(|harness| {
                let name = harness.name.clone();
                let selected = chat
                    .harness
                    .as_deref()
                    .map_or(harness.default, |picked| picked == name);
                let mut item = MenuItem::new(&name).checked(selected);
                if harness.default {
                    item = item.badge("Default");
                }
                item.on_select_tx(move |s: &mut Desktop, tx| {
                    s.open_picker = None;
                    tx.focus("draft", false);
                    if !selected && let Some(chat) = &mut s.new_chat {
                        chat.harness = Some(name.clone());
                        chat.options.clear();
                    }
                })
            }),
        );
    }
    (
        menu.build(&t.k, layer).min_w(0.0).shrink(),
        picker_width(name.unwrap_or(label), t),
    )
}
