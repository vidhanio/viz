//! Four-group sidebar thread context menu and nested submenus (Snooze,
//! Auto-settle behavior, Move to project, Copy).

use rune::{El, el};
use rune_kit::{Menu, MenuEntry, MenuItem, Placement};
use viz_client::protocol::{
    AutoSettle, Position, Project, Shelf, ThreadId, ThreadSummary, TriageAction,
};

use crate::{
    app::Desktop,
    model::{ContextSubmenu, ProjectFilter, SidebarEdit, SnoozePreset},
    theme::Theme,
    ui,
    update::{Msg, SidebarMsg},
};

/// Minimum width of a thread card context menu (points).
const CONTEXT_MENU_WIDTH: f32 = 208.0;

/// Mounts the full 4-group thread context menu into `layer` when open for
/// `key`.
#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive 4-group thread context menu definition"
)]
pub(super) fn thread_context_menu(
    s: &Desktop,
    thread: &ThreadSummary,
    key: &str,
    shelf: Shelf,
    now: i64,
    t: &Theme,
    layer: &mut Vec<El>,
) {
    let m = &s.model;
    let Some(context) = m
        .sidebar
        .menu
        .as_ref()
        .filter(|context| context.key == key && m.connected())
    else {
        return;
    };
    let id = thread.id;
    let project = thread
        .project
        .as_deref()
        .and_then(|path| m.cache().project(path));
    let branch = thread
        .worktree
        .as_ref()
        .and_then(|wt| wt.branch.as_deref())
        .or_else(|| project.and_then(|p| p.branch.as_deref()));

    let mut menu = Menu::new(format!("{key}-context"))
        .trigger(el().key(format!("{key}-ctx-anchor")).size(0.0, 0.0))
        .placement(Placement::BottomStart)
        .min_width(CONTEXT_MENU_WIDTH)
        .open(true)
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseRowMenu)));
    if let Some((x, y)) = context.point {
        menu = menu.at(x, y);
    }

    let new_label = match (branch, project) {
        (Some(branch), _) => format!("New thread on {branch}"),
        (None, Some(project)) => format!("New thread in {}", project.name),
        (None, None) => "New thread".to_owned(),
    };
    let new_project = thread.project.clone();
    let new_branch = branch.map(str::to_owned);
    let pinned = thread.lifecycle.pin.is_some();
    let settled = shelf == Shelf::Settled;
    let working = shelf == Shelf::Working;
    let snoozed = shelf == Shelf::Snoozed;
    let unread = thread.lifecycle.unread;
    let archived = thread.lifecycle.archived;

    menu = menu
        .item(
            MenuItem::new(new_label)
                .icon(&ui::COMPOSE)
                .disabled(m.selection.new_thread.is_some())
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::NewThreadIn {
                            project: new_project.clone(),
                            branch: new_branch.clone(),
                        }),
                        tx,
                    );
                }),
        )
        .item(
            MenuItem::new(if pinned { "Unpin thread" } else { "Pin thread" })
                .icon(if pinned { &ui::PIN_OFF } else { &ui::PIN })
                .on_select_tx(move |s: &mut Desktop, tx| {
                    let (action, undo_title) = if pinned {
                        (TriageAction::Unpin, Some("Thread unpinned"))
                    } else {
                        (
                            TriageAction::Pin {
                                position: Position::Top,
                            },
                            None,
                        )
                    };
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::Triage {
                            thread: id,
                            action,
                            undo_title,
                            now,
                        }),
                        tx,
                    );
                }),
        )
        .item(
            MenuItem::new(if settled {
                "Reopen thread"
            } else {
                "Settle thread"
            })
            .icon(if settled {
                &ui::ROTATE_CCW
            } else {
                &ui::CIRCLE_CHECK
            })
            .disabled(working && !settled)
            .on_select_tx(move |s: &mut Desktop, tx| {
                let (action, undo_title) = if settled {
                    (TriageAction::Reopen, None)
                } else {
                    (TriageAction::Settle, Some("Thread settled"))
                };
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread: id,
                        action,
                        undo_title,
                        now,
                    }),
                    tx,
                );
            }),
        )
        .item(
            MenuItem::new("Snooze")
                .icon(&ui::CLOCK)
                .submenu(
                    snooze_entries(id, snoozed, now),
                    context.submenu == Some(ContextSubmenu::Snooze),
                )
                .on_select(|s: &mut Desktop| {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleSubmenu(
                        ContextSubmenu::Snooze,
                    )));
                }),
        )
        .separator()
        .item(
            MenuItem::new("Rename thread")
                .icon(&ui::PENCIL)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::StartInlineRename(id)), tx);
                }),
        )
        .item(
            MenuItem::new("Regenerate title")
                .icon(&ui::REFRESH_CW)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::RetitleThread(id)), tx);
                }),
        )
        .item(
            MenuItem::new(if unread { "Mark read" } else { "Mark unread" })
                .icon(if unread { &ui::MAIL_OPEN } else { &ui::MAIL })
                .on_select_tx(move |s: &mut Desktop, tx| {
                    let action = if unread {
                        TriageAction::MarkRead
                    } else {
                        TriageAction::MarkUnread
                    };
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::Triage {
                            thread: id,
                            action,
                            undo_title: None,
                            now,
                        }),
                        tx,
                    );
                }),
        );

    let (filter_label, target_filter) = match project {
        Some(project)
            if m.sidebar.project_filter == ProjectFilter::Project(project.path.clone()) =>
        {
            ("Clear project filter".to_owned(), ProjectFilter::All)
        }
        Some(project) => (
            format!("Filter by {}", project.name),
            ProjectFilter::Project(project.path.clone()),
        ),
        None if m.sidebar.project_filter == ProjectFilter::NoProject => {
            ("Clear project filter".to_owned(), ProjectFilter::All)
        }
        None => ("Filter by No project".to_owned(), ProjectFilter::NoProject),
    };
    menu = menu
        .item(
            MenuItem::new(filter_label)
                .icon(&ui::LIST_FILTER)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    tx.instant();
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::SetProjectFilter(target_filter.clone())),
                        tx,
                    );
                }),
        )
        .item(
            MenuItem::new("Auto-settle behavior")
                .icon(&ui::TIMER)
                .submenu(
                    auto_settle_entries(id, thread.lifecycle.auto_settle, now),
                    context.submenu == Some(ContextSubmenu::AutoSettle),
                )
                .on_select(|s: &mut Desktop| {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleSubmenu(
                        ContextSubmenu::AutoSettle,
                    )));
                }),
        )
        .item(
            MenuItem::new("Fork thread")
                .icon(&ui::GIT_FORK)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::ForkThread(id)), tx);
                }),
        )
        .separator();

    if m.cache().has_projects() || thread.project.is_some() {
        let destinations = std::iter::once(destination(thread, None))
            .chain(
                m.cache()
                    .projects()
                    .map(|project| destination(thread, Some(project))),
            )
            .map(MenuEntry::item)
            .collect();
        menu = menu.item(
            MenuItem::new("Move to project")
                .icon(&ui::FOLDER)
                .disabled(thread.worktree.is_some())
                .submenu(
                    destinations,
                    context.submenu == Some(ContextSubmenu::MoveToProject),
                )
                .on_select(|s: &mut Desktop| {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleSubmenu(
                        ContextSubmenu::MoveToProject,
                    )));
                }),
        );
    }

    menu = menu
        .item(
            MenuItem::new("Copy")
                .icon(&ui::COPY)
                .submenu(
                    copy_entries(thread, branch),
                    context.submenu == Some(ContextSubmenu::Copy),
                )
                .on_select(|s: &mut Desktop| {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleSubmenu(
                        ContextSubmenu::Copy,
                    )));
                }),
        )
        .separator()
        .item(
            MenuItem::new(if archived {
                "Unarchive thread"
            } else {
                "Archive thread"
            })
            .icon(&ui::ARCHIVE)
            .on_select_tx(move |s: &mut Desktop, tx| {
                let (action, undo_title) = if archived {
                    (TriageAction::Unarchive, None)
                } else {
                    (TriageAction::Archive, Some("Thread archived"))
                };
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread: id,
                        action,
                        undo_title,
                        now,
                    }),
                    tx,
                );
            }),
        )
        .item(
            MenuItem::new("Delete")
                .icon(&ui::TRASH)
                .danger(true)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::OpenDeleteThread(id)), tx);
                }),
        );

    if thread.worktree.is_some() {
        menu = menu.item(
            MenuItem::new("Delete with worktree")
                .icon(&ui::TRASH)
                .danger(true)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::OpenEdit {
                            edit: SidebarEdit::DeleteWithWorktree(id),
                            initial_value: String::new(),
                        }),
                        tx,
                    );
                }),
        );
    }

    let _ = menu.build(&t.overlay, layer);
}

fn snooze_entries(thread: ThreadId, snoozed: bool, now: i64) -> Vec<MenuEntry> {
    let mut entries: Vec<MenuEntry> = SnoozePreset::ALL
        .into_iter()
        .map(|preset| {
            MenuEntry::item(MenuItem::new(preset.label()).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::SnoozePreset {
                            thread,
                            preset,
                            now,
                        }),
                        tx,
                    );
                },
            ))
        })
        .collect();
    if snoozed {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(MenuItem::new("Unsnooze").on_select_tx(
            move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread,
                        action: TriageAction::Wake,
                        undo_title: None,
                        now,
                    }),
                    tx,
                );
            },
        )));
    }
    entries
}

fn auto_settle_entries(thread: ThreadId, current: AutoSettle, now: i64) -> Vec<MenuEntry> {
    [
        ("Default", AutoSettle::Follow),
        ("Never auto-settle", AutoSettle::Never),
    ]
    .into_iter()
    .map(|(label, mode)| {
        MenuEntry::item(MenuItem::new(label).checked(current == mode).on_select_tx(
            move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::Triage {
                        thread,
                        action: TriageAction::AutoSettle { mode },
                        undo_title: None,
                        now,
                    }),
                    tx,
                );
            },
        ))
    })
    .collect()
}

fn copy_entries(thread: &ThreadSummary, branch: Option<&str>) -> Vec<MenuEntry> {
    let thread_id = thread.id.0.to_string();
    let mut entries = vec![MenuEntry::item(
        MenuItem::new("Copy thread ID").on_select_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Sidebar(SidebarMsg::CopyText {
                    label: "Thread ID",
                    value: thread_id.clone(),
                }),
                tx,
            );
        }),
    )];
    if let Some(branch) = branch {
        let value = branch.to_owned();
        entries.push(MenuEntry::item(
            MenuItem::new("Copy branch name").on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::CopyText {
                        label: "Branch name",
                        value: value.clone(),
                    }),
                    tx,
                );
            }),
        ));
    }
    if let Some(worktree) = &thread.worktree {
        let value = worktree.path.clone();
        entries.push(MenuEntry::item(
            MenuItem::new("Copy worktree path").on_select_tx(move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::CopyText {
                        label: "Worktree path",
                        value: value.clone(),
                    }),
                    tx,
                );
            }),
        ));
    }
    entries
}

fn destination(thread: &ThreadSummary, project: Option<&Project>) -> MenuItem {
    let id = thread.id;
    let current = thread.project.as_deref() == project.map(|project| project.path.as_str());
    let project_id = project.map(|project| project.path.clone());
    MenuItem::new(project.map_or("No project", |project| project.name.as_str()))
        .checked(current)
        .disabled(current)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.dispatch(
                Msg::Sidebar(SidebarMsg::MoveThreadToProject {
                    thread: id,
                    project: project_id.clone(),
                }),
                tx,
            );
        })
}
