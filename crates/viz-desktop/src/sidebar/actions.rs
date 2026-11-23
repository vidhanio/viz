//! Sidebar row context menus and modal rename/delete confirmation dialogs.

use rune::El;
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuEntry, MenuItem, Placement};
use viz_client::protocol::{Project, ThreadSummary};

use crate::{
    app::Desktop,
    model::{ContextMenu, SidebarEdit},
    theme::{Size, Theme},
    ui::{self, DialogForm},
    update::{Msg, SelectionMsg, SidebarMsg},
};

/// Mounts the active rename or delete confirmation dialog, if any.
pub(super) fn dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let m = &s.model;
    let Some(edit) = &m.sidebar.edit else {
        return;
    };
    let field = Some((&s.widgets.sidebar_dialog, "Name"));
    let form = match edit {
        SidebarEdit::RenameProject(_) | SidebarEdit::RenameThread(_) => DialogForm {
            title: if matches!(edit, SidebarEdit::RenameProject(_)) {
                "Rename project"
            } else {
                "Rename thread"
            },
            subject: None,
            detail: None,
            field,
            action: "Save",
            variant: ButtonVariant::Submit,
        },
        SidebarEdit::RemoveProject(id) => DialogForm {
            title: "Remove project?",
            subject: m.cache().project(id).map(|p| p.path.as_str()),
            detail: None,
            field: None,
            action: "Remove project",
            variant: ButtonVariant::Danger,
        },
        SidebarEdit::DeleteThread(id) => DialogForm {
            title: "Delete thread?",
            subject: Some(m.title(*id)),
            detail: Some(
                if m.summary(*id)
                    .is_some_and(|thread| thread.worktree.is_some())
                {
                    "This can’t be undone. Its worktree stays."
                } else {
                    "This can’t be undone."
                },
            ),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
        SidebarEdit::DeleteWithWorktree(id) => DialogForm {
            title: "Delete thread and worktree?",
            subject: m
                .summary(*id)
                .and_then(|thread| thread.worktree.as_ref())
                .map(|worktree| worktree.path.as_str()),
            detail: Some(
                "The worktree must have no uncommitted changes. Its branch is kept. This can’t be undone.",
            ),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
    };
    form.build(
        |s: &mut Desktop| &mut s.widgets.sidebar_dialog,
        |s| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseEdit)),
        |s, tx| {
            let value = s.widgets.sidebar_dialog.text.clone();
            s.dispatch(Msg::Sidebar(SidebarMsg::EditValueChanged(value)), tx);
            s.dispatch(Msg::Sidebar(SidebarMsg::SubmitEdit), tx);
        },
        t,
        layer,
    );
}

/// Whether the row `key`'s menu is open.
pub(super) fn menu_open(s: &Desktop, key: &str) -> bool {
    s.model.connected()
        && s.model
            .sidebar
            .menu
            .as_ref()
            .is_some_and(|context| context.key == key)
}

fn menu<'a>(s: &'a Desktop, key: &str, t: &Theme) -> (Menu, Option<&'a ContextMenu>) {
    let m = &s.model;
    let context = m
        .sidebar
        .menu
        .as_ref()
        .filter(|context| context.key == key && m.connected());
    let target = key.to_owned();
    let trigger = ui::icon_button(
        &ui::MORE,
        "Actions",
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .disabled(!m.connected())
    .on_press(move |s: &mut Desktop| {
        s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleRowMenu(target.clone())));
    })
    .build(&t.k)
    .key(format!("{key}-actions"));
    let mut menu = Menu::new(format!("{key}-context"))
        .trigger(trigger)
        .placement(Placement::BottomEnd)
        .min_width(ui::MENU_WIDTH)
        .open(context.is_some())
        .on_close(|s: &mut Desktop| s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseRowMenu)));
    if let Some((x, y)) = context.and_then(|context| context.point) {
        menu = menu.at(x, y);
    }
    (menu, context)
}

/// A top-level thread's actions menu.
pub(super) fn thread_menu(
    s: &Desktop,
    thread: &ThreadSummary,
    key: &str,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let m = &s.model;
    let (mut menu, context) = menu(s, key, t);
    let Some(context) = context else {
        return menu.build(&t.overlay, layer);
    };
    let id = thread.id;
    let pinned = thread.lifecycle.pin.is_some();
    menu = menu.item(
        MenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_select_tx(
            move |s: &mut Desktop, tx| {
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::PinThread {
                        thread: id,
                        pinned: !pinned,
                    }),
                    tx,
                );
            },
        ),
    );
    let archived = thread.lifecycle.archived;
    menu = menu
        .item(
            MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                let title = s
                    .model
                    .summary(id)
                    .map_or_else(String::new, |thread| thread.title.clone());
                s.dispatch(
                    Msg::Sidebar(SidebarMsg::OpenEdit {
                        edit: SidebarEdit::RenameThread(id),
                        initial_value: title,
                    }),
                    tx,
                );
            }),
        )
        .item(
            MenuItem::new(if archived { "Unarchive" } else { "Archive" }).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::ArchiveThread {
                            thread: id,
                            archived: !archived,
                        }),
                        tx,
                    );
                },
            ),
        );
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
                .disabled(thread.worktree.is_some())
                .submenu(destinations, context.moving)
                .on_select(|s: &mut Desktop| {
                    s.dispatch_local(Msg::Sidebar(SidebarMsg::ToggleMoveSubmenu));
                }),
        );
    }
    menu = menu
        .separator()
        .item(
            MenuItem::new("Delete")
                .danger(true)
                .on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(Msg::Sidebar(SidebarMsg::OpenDeleteThread(id)), tx);
                }),
        );
    if thread.worktree.is_some() {
        menu = menu.item(
            MenuItem::new("Delete with worktree")
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
    menu.build(&t.overlay, layer)
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

pub(super) fn project_menu(
    s: &Desktop,
    project: &Project,
    key: &str,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let (mut menu, context) = menu(s, key, t);
    if context.is_some() {
        let new = project.path.clone();
        let rename = project.path.clone();
        let remove = project.path.clone();
        let name = project.name.clone();
        menu = menu
            .item(
                MenuItem::new("New thread")
                    .disabled(s.model.selection.new_thread.is_some())
                    .on_select_tx(move |s: &mut Desktop, tx| {
                        s.dispatch_local(Msg::Sidebar(SidebarMsg::CloseRowMenu));
                        s.dispatch(
                            Msg::Selection(SelectionMsg::CreateInProject(new.clone())),
                            tx,
                        );
                    }),
            )
            .item(
                MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::OpenEdit {
                            edit: SidebarEdit::RenameProject(rename.clone()),
                            initial_value: name.clone(),
                        }),
                        tx,
                    );
                }),
            )
            .separator()
            .item(MenuItem::new("Remove project").danger(true).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.dispatch(
                        Msg::Sidebar(SidebarMsg::OpenEdit {
                            edit: SidebarEdit::RemoveProject(remove.clone()),
                            initial_value: String::new(),
                        }),
                        tx,
                    );
                },
            ));
    }
    menu.build(&t.overlay, layer)
}
