//! Where a thread runs in its Git project: the project's own checkout or a
//! linked worktree. Draft and active-thread checkout controls share their
//! labels.

use std::path::Path;

use rune::{El, Icon, Tx, icon, row, text};
use rune_kit::{Menu, MenuItem, Placement};
use viz_client::protocol::{Project, ThreadId, WorktreeTarget};

use crate::{
    app::Desktop,
    model::{Checkouts, NewThread},
    theme::{HAIRLINE, ICON_SMALL, RADIUS_CONTROL, S6, S8, Size, Theme},
    ui,
    update::{ComposerMsg, Msg, SelectionMsg},
};

/// The `open_picker` value of a draft's checkout picker.
pub const DRAFT_CHECKOUT: &str = "draft-checkout";
/// The `open_picker` value of the toolbar's checkout menu.
pub const THREAD_CHECKOUT: &str = "thread-checkout";
/// The most branches a draft offers as a new worktree's base; the rest are
/// reachable from the agent's tools.
const BASES: usize = 12;

/// A draft's checkout picker, for a Git project.
pub fn draft(s: &Desktop, thread: &NewThread, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    let project = m.project().filter(|project| project.git)?;
    let open = m.composer.open_picker.as_deref() == Some(DRAFT_CHECKOUT) && !thread.sending;
    let checkouts = m.checkouts(&project.path);
    let (label, glyph) = draft_label(thread, checkouts);
    let trigger = trigger(DRAFT_CHECKOUT, &label, glyph, open, thread.sending, true, t)
        .on_press_tx(toggle(DRAFT_CHECKOUT, &project.path));
    let mut menu = Menu::new(format!("{DRAFT_CHECKOUT}-picker"))
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Composer(ComposerMsg::ClosePicker)))
        .trigger(trigger);
    if open {
        let current = thread.worktree.as_ref();
        let choice =
            |label: &str, detail: Option<&str>, glyph: &'static Icon, target: WorktreeTarget| {
                let picked = current.unwrap_or(&WorktreeTarget::Local) == &target;
                let mut item = MenuItem::new(label).icon(glyph).checked(picked);
                if let Some(detail) = detail {
                    item = item.description(detail);
                }
                item.on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Selection(SelectionMsg::ChooseDraftWorktree(target.clone())),
                        tx,
                    );
                })
            };
        let head = project.branch.as_deref().unwrap_or("HEAD");
        menu = menu
            .item(choice(
                "Current checkout",
                Some(head),
                &ui::GIT_BRANCH,
                WorktreeTarget::Local,
            ))
            .item(choice(
                "New worktree",
                Some(&format!("New branch from {head}")),
                &ui::GIT_BRANCH_PLUS,
                WorktreeTarget::New {
                    base: None,
                    branch: None,
                },
            ));
        if let Some(checkouts) = checkouts {
            let bases: Vec<_> = checkouts
                .branches
                .iter()
                .filter(|branch| Some(branch.name.as_str()) != project.branch.as_deref())
                .take(BASES)
                .collect();
            if !bases.is_empty() {
                menu = menu.separator().group_label("New worktree from");
                menu = ui::menu_items(
                    menu,
                    bases.into_iter().map(|branch| {
                        choice(
                            &branch.name,
                            None,
                            &ui::GIT_BRANCH_PLUS,
                            WorktreeTarget::New {
                                base: Some(branch.name.clone()),
                                branch: None,
                            },
                        )
                    }),
                );
            }
            let linked: Vec<_> = checkouts.worktrees.iter().filter(|w| !w.main).collect();
            if !linked.is_empty() {
                menu = menu.separator().group_label("Previous worktree");
                menu = ui::menu_items(
                    menu,
                    linked.into_iter().map(|worktree| {
                        choice(
                            &worktree_label(Some(checkouts), &worktree.path),
                            None,
                            &ui::GIT_BRANCH,
                            WorktreeTarget::Existing {
                                path: worktree.path.clone(),
                            },
                        )
                    }),
                );
            }
        }
    }
    Some(menu.build(&t.overlay, layer).min_w(0.0).shrink())
}

/// A draft checkout picker's label and icon.
fn draft_label(thread: &NewThread, checkouts: Option<&Checkouts>) -> (String, &'static Icon) {
    match &thread.worktree {
        None | Some(WorktreeTarget::Local) => ("Current checkout".to_owned(), &ui::GIT_BRANCH),
        Some(WorktreeTarget::New { base: None, .. }) => {
            ("New worktree".to_owned(), &ui::GIT_BRANCH_PLUS)
        }
        Some(WorktreeTarget::New {
            base: Some(base), ..
        }) => (format!("New worktree from {base}"), &ui::GIT_BRANCH_PLUS),
        Some(WorktreeTarget::Existing { path }) => {
            (worktree_label(checkouts, path), &ui::GIT_BRANCH)
        }
    }
}

/// The open thread's branch in the toolbar, for a Git project; its menu
/// moves the thread between the project checkout and worktrees.
pub fn thread(s: &Desktop, t: &Theme, layer: &mut Vec<El>) -> Option<El> {
    let m = &s.model;
    let id = m.selection.selected?;
    let thread = m.summary(id)?;
    let project = thread
        .project
        .as_deref()
        .and_then(|path| m.cache().project(path))
        .filter(|project| project.git)?;
    let open = m.composer.open_picker.as_deref() == Some(THREAD_CHECKOUT) && m.connected();
    let worktree = thread.worktree.as_ref();
    let branch = worktree.map_or(project.branch.as_deref(), |worktree| {
        worktree.branch.as_deref()
    });
    let label = if worktree.is_some() {
        branch.unwrap_or("Previous worktree")
    } else {
        "Current checkout"
    };
    let glyph = if worktree.is_some() {
        &ui::GIT_BRANCH_PLUS
    } else {
        &ui::GIT_BRANCH
    };
    let trigger = trigger(
        THREAD_CHECKOUT,
        label,
        glyph,
        open,
        !m.connected() || m.busy(),
        false,
        t,
    )
    .on_press_tx(toggle(THREAD_CHECKOUT, &project.path));
    let mismatch = worktree.is_none()
        && m.git
            .status
            .as_ref()
            .and_then(|status| status.branch.as_deref())
            != branch;
    let trigger = if m.busy() {
        ui::with_tooltip(trigger, "Stop the thread to move it", None)
    } else if mismatch && m.git.status.is_some() {
        ui::with_tooltip(
            trigger,
            "The checkout branch changed outside this thread",
            None,
        )
    } else {
        trigger
    };
    let mut menu = Menu::new(format!("{THREAD_CHECKOUT}-menu"))
        .placement(Placement::Bottom)
        .min_width(ui::MENU_WIDTH)
        .open(open)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Composer(ComposerMsg::ClosePicker)))
        .trigger(trigger);
    if open {
        menu = move_items(menu, s, id, project, worktree.map(|w| w.path.as_str()));
    }
    Some(menu.build(&t.overlay, layer).min_w(0.0).shrink())
}

fn move_items(
    mut menu: Menu,
    s: &Desktop,
    id: ThreadId,
    project: &Project,
    current: Option<&str>,
) -> Menu {
    let busy = s.model.busy();
    let action = |label: &str,
                  detail: Option<&str>,
                  glyph: &'static Icon,
                  here: bool,
                  target: WorktreeTarget| {
        let mut item = MenuItem::new(label).icon(glyph);
        if let Some(detail) = detail {
            item = item.description(detail);
        }
        item.checked(here)
            .disabled(here || busy)
            .on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Selection(SelectionMsg::MoveThread {
                        thread: id,
                        worktree: target.clone(),
                    }),
                    tx,
                );
            })
    };
    menu = menu.group_label(if busy {
        "Stop the thread to move it"
    } else {
        "Move thread to"
    });
    menu = menu
        .item(action(
            "Current checkout",
            project.branch.as_deref(),
            &ui::GIT_BRANCH,
            current.is_none(),
            WorktreeTarget::Local,
        ))
        .item(
            MenuItem::new("New worktree")
                .icon(&ui::GIT_BRANCH_PLUS)
                .disabled(busy)
                .on_select_tx(|s: &mut Desktop, tx| {
                    s.widgets.git_query = rune::TextEdit::new("");
                    super::dispatch(s, crate::update::GitMsg::NewWorktree, tx);
                    tx.focus_when_shown("git-branch-search", false);
                }),
        );
    if let Some(checkouts) = s.model.checkouts(&project.path) {
        for worktree in checkouts.worktrees.iter().filter(|worktree| !worktree.main) {
            menu = menu.item(action(
                &worktree_label(Some(checkouts), &worktree.path),
                None,
                &ui::GIT_BRANCH,
                current == Some(worktree.path.as_str()),
                WorktreeTarget::Existing {
                    path: worktree.path.clone(),
                },
            ));
        }
    }
    menu
}

/// A worktree's branch, or its directory name when detached or unknown.
fn worktree_label(checkouts: Option<&Checkouts>, path: &str) -> String {
    checkouts
        .and_then(|checkouts| checkouts.worktrees.iter().find(|w| w.path == path))
        .and_then(|worktree| worktree.branch.clone())
        .unwrap_or_else(|| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(path)
                .to_owned()
        })
}

/// Opens or closes picker `key`, loading `project`'s checkouts on open.
fn toggle(key: &'static str, project: &str) -> impl Fn(&mut Desktop, &mut Tx) + 'static {
    let project = project.to_owned();
    move |s: &mut Desktop, tx| {
        s.dispatch(
            Msg::Composer(ComposerMsg::ToggleCheckoutPicker {
                key: key.to_owned(),
                project: project.clone(),
            }),
            tx,
        );
    }
}

/// A picker trigger: bordered beside a draft's project picker, ghost in the
/// toolbar.
fn trigger(
    key: &str,
    label: &str,
    glyph: &'static Icon,
    open: bool,
    disabled: bool,
    bordered: bool,
    t: &Theme,
) -> El {
    let mut trigger = row()
        .key(format!("{key}-trigger"))
        .role("combobox")
        .label(format!("Checkout: {label}"))
        .expanded(open)
        .focusable(!disabled)
        .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
        .hover_group()
        .h(Size::Default.height())
        .px(S8)
        .gap(S6)
        .center_y()
        .radius(RADIUS_CONTROL)
        .min_w(0.0)
        .max_w(ui::MODEL_TRIGGER_MAX);
    if bordered {
        trigger = trigger.fill(t.surface).border(HAIRLINE, t.hairline);
    }
    let ink = if bordered { t.text } else { t.muted };
    trigger
        .child(
            icon(glyph, ICON_SMALL)
                .color(ink)
                .group_color(t.text)
                .a11y_hidden(),
        )
        .child(
            text(label)
                .font(t.ui_medium)
                .color(ink)
                .group_color(t.text)
                .min_w(0.0)
                .shrink()
                .truncate()
                .a11y_hidden(),
        )
        .child(
            icon(&ui::CHEVRON_OPEN, ICON_SMALL)
                .key("chevron")
                .icon_rotation(if open { std::f32::consts::PI } else { 0.0 })
                .color(t.muted)
                .group_color(t.text)
                .a11y_hidden(),
        )
}
