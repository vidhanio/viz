use rune::{El, FileDialogOptions, PointerPress, Tx};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuEntry, MenuItem, Placement};
use viz_client::protocol::{Project, ThreadId, ThreadSummary};

use crate::{
    app::Desktop,
    server::{Command, ProjectChange, ThreadChange},
    theme::{Size, Theme},
    ui::{self, DialogForm},
};

#[derive(Debug)]
pub(super) struct Context {
    key: String,
    point: Option<(f32, f32)>,
    moving: bool,
}

impl Context {
    fn new(key: &str) -> Self {
        Self {
            key: key.into(),
            point: None,
            moving: false,
        }
    }

    /// A thread's row menu as a secondary press opened it at `(x, y)`, its
    /// Move to project submenu open, for a preview scene.
    pub(super) fn preview(key: &str, x: f32, y: f32) -> Self {
        Self {
            point: Some((x, y)),
            moving: true,
            ..Self::new(key)
        }
    }

    pub(super) fn at(key: &str, point: PointerPress) -> Self {
        Self {
            point: Some((point.x, point.y)),
            ..Self::new(key)
        }
    }
}

#[derive(Debug)]
pub(super) enum Edit {
    RenameProject(String),
    RemoveProject(String),
    RenameThread(ThreadId),
    DeleteThread(ThreadId),
}

fn open_edit(s: &mut Desktop, edit: Edit, value: &str, tx: &mut Tx) {
    if !s.connected() {
        return;
    }
    s.navigation.menu = None;
    s.navigation.edit = Some(edit);
    s.navigation.value.replace_text(value);
    s.navigation.value.set_selection(0, value.len());
    tx.focus_when_shown("edit-value", false);
}

/// Picks a folder with the OS dialog and adds it as a project; the server
/// names it after the folder (rename it from its menu).
pub(super) fn add_project(s: &mut Desktop, tx: &mut Tx) {
    if !s.connected() {
        return;
    }
    s.navigation.menu = None;
    let options = FileDialogOptions {
        title: "Add project".into(),
        ..FileDialogOptions::default()
    };
    tx.open_folder(options, |s: &mut Desktop, result, tx| match result {
        Ok(Some(path)) => {
            s.send(
                Command::Project(ProjectChange::Add(path.to_string_lossy().into_owned())),
                tx,
            );
        }
        Ok(None) => {}
        Err(error) => s.toast(
            tx,
            "Couldn’t open the folder picker",
            Some(error.to_string()),
        ),
    });
}

/// Asks to delete a thread.
pub(super) fn delete_thread(s: &mut Desktop, id: ThreadId, tx: &mut Tx) {
    if s.summary(id).is_some() {
        open_edit(s, Edit::DeleteThread(id), "", tx);
    }
}

fn edit_send_input(s: &mut Desktop, tx: &mut Tx) {
    if !s.connected() {
        return;
    }
    let Some(edit) = s.navigation.edit.as_ref() else {
        return;
    };
    let value = s.navigation.value.text.trim();
    if value.is_empty() && !matches!(edit, Edit::RemoveProject(_) | Edit::DeleteThread(_)) {
        return;
    }
    let command = match edit {
        Edit::RenameProject(id) => Command::Project(ProjectChange::Rename {
            id: id.clone(),
            name: value.into(),
        }),
        Edit::RemoveProject(id) => Command::Project(ProjectChange::Remove(id.clone())),
        Edit::RenameThread(id) => Command::Update(*id, ThreadChange::Title(value.into())),
        Edit::DeleteThread(id) => Command::Delete(*id),
    };
    if s.send(command, tx) {
        s.navigation.edit = None;
    }
}

pub(super) fn dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let Some(edit) = &s.navigation.edit else {
        return;
    };
    let field = Some((&s.navigation.value, "Name"));
    let form = match edit {
        Edit::RenameProject(_) | Edit::RenameThread(_) => DialogForm {
            title: if matches!(edit, Edit::RenameProject(_)) {
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
        Edit::RemoveProject(id) => DialogForm {
            title: "Remove project?",
            subject: s
                .projects
                .iter()
                .find(|p| p.path == *id)
                .map(|p| p.path.as_str()),
            detail: None,
            field: None,
            action: "Remove project",
            variant: ButtonVariant::Danger,
        },
        Edit::DeleteThread(id) => DialogForm {
            title: "Delete thread?",
            subject: Some(s.title(*id)),
            detail: Some("This can’t be undone."),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
    };
    form.build(
        |s: &mut Desktop| &mut s.navigation.value,
        |s| s.navigation.edit = None,
        edit_send_input,
        t,
        layer,
    );
}

/// Whether the row `key`'s menu is open.
pub(super) fn menu_open(s: &Desktop, key: &str) -> bool {
    s.connected()
        && s.navigation
            .menu
            .as_ref()
            .is_some_and(|context| context.key == key)
}

fn menu<'a>(s: &'a Desktop, key: &str, t: &Theme) -> (Menu, Option<&'a Context>) {
    let context = s
        .navigation
        .menu
        .as_ref()
        .filter(|context| context.key == key && s.connected());
    let target = key.to_owned();
    let trigger = ui::icon_button(
        &ui::MORE,
        "Actions",
        IconButtonVariant::GhostMuted,
        Size::Small,
    )
    .disabled(!s.connected())
    .on_press(move |s: &mut Desktop| {
        s.navigation.menu = if s
            .navigation
            .menu
            .as_ref()
            .is_some_and(|context| context.key == target)
        {
            None
        } else {
            Some(Context::new(&target))
        };
    })
    .build(&t.k)
    .key(format!("{key}-actions"));
    let mut menu = Menu::new(format!("{key}-context"))
        .trigger(trigger)
        .placement(Placement::BottomEnd)
        .min_width(ui::MENU_WIDTH)
        .open(context.is_some())
        .on_close(|s: &mut Desktop| s.navigation.menu = None);
    if let Some((x, y)) = context.and_then(|context| context.point) {
        menu = menu.at(x, y);
    }
    (menu, context)
}

/// A top-level thread’s actions.
pub(super) fn thread_menu(
    s: &Desktop,
    thread: &ThreadSummary,
    key: &str,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let (mut menu, context) = menu(s, key, t);
    let Some(context) = context else {
        return menu.build(&t.overlay, layer);
    };
    let id = thread.id;
    let pinned = thread.pinned;
    menu = menu.item(
        MenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_select_tx(
            move |s: &mut Desktop, tx| {
                s.navigation.menu = None;
                if s.connected() {
                    s.send(Command::Update(id, ThreadChange::Pin(!pinned)), tx);
                }
            },
        ),
    );
    let archived = thread.archived;
    menu = menu
        .item(
            MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                let title = s
                    .summary(id)
                    .map_or_else(String::new, |thread| thread.title.clone());
                open_edit(s, Edit::RenameThread(id), &title, tx);
            }),
        )
        .item(
            MenuItem::new(if archived { "Unarchive" } else { "Archive" }).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.navigation.menu = None;
                    s.send(Command::Update(id, ThreadChange::Archive(!archived)), tx);
                },
            ),
        );
    if !s.projects.is_empty() || thread.project.is_some() {
        let destinations = std::iter::once(destination(thread, None))
            .chain(
                s.projects
                    .iter()
                    .map(|project| destination(thread, Some(project))),
            )
            .map(MenuEntry::item)
            .collect();
        menu = menu.item(
            MenuItem::new("Move to project")
                .submenu(destinations, context.moving)
                .on_select(|s: &mut Desktop| {
                    if let Some(context) = &mut s.navigation.menu {
                        context.moving = !context.moving;
                    }
                }),
        );
    }
    menu = menu.separator().item(
        MenuItem::new("Delete")
            .danger(true)
            .on_select_tx(move |s: &mut Desktop, tx| delete_thread(s, id, tx)),
    );
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
            s.navigation.menu = None;
            if let Some(project) = &project_id {
                s.navigation.expand_project(project);
            }
            s.send(
                Command::Update(id, ThreadChange::Project(project_id.clone())),
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
                    .disabled(s.new_thread.is_some())
                    .on_select_tx(move |s: &mut Desktop, tx| {
                        s.navigation.menu = None;
                        s.create_in_project(&new, tx);
                    }),
            )
            .item(
                MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                    open_edit(s, Edit::RenameProject(rename.clone()), &name, tx);
                }),
            )
            .separator()
            .item(MenuItem::new("Remove project").danger(true).on_select_tx(
                move |s: &mut Desktop, tx| {
                    open_edit(s, Edit::RemoveProject(remove.clone()), "", tx);
                },
            ));
    }
    menu.build(&t.overlay, layer)
}
