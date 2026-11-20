use rune::{El, FileDialogOptions, PointerPress, Tx};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuEntry, MenuItem, Placement};
use viz_client::protocol::{ConversationId, ConversationKind, ConversationSummary, Project};

use crate::{
    app::Desktop,
    backend::{ChatChange, Command, ProjectChange},
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

    /// A chat's row menu as a secondary press opened it at `(x, y)`, its
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
    RenameChat(ConversationId),
    DeleteChat(ConversationId),
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

/// Picks a folder with the OS dialog and adds it as a project; the backend
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

/// Asks to delete chat `id` and its subagents. The orchestrator can't be
/// deleted.
pub(super) fn delete_chat(s: &mut Desktop, id: ConversationId, tx: &mut Tx) {
    if s.summary(id)
        .is_some_and(|chat| chat.kind != ConversationKind::Orchestrator)
    {
        open_edit(s, Edit::DeleteChat(id), "", tx);
    }
}

fn edit_submit(s: &mut Desktop, tx: &mut Tx) {
    if !s.connected() {
        return;
    }
    let Some(edit) = s.navigation.edit.as_ref() else {
        return;
    };
    let value = s.navigation.value.text.trim();
    if value.is_empty() && !matches!(edit, Edit::RemoveProject(_) | Edit::DeleteChat(_)) {
        return;
    }
    let command = match edit {
        Edit::RenameProject(id) => Command::Project(ProjectChange::Rename {
            id: id.clone(),
            name: value.into(),
        }),
        Edit::RemoveProject(id) => Command::Project(ProjectChange::Remove(id.clone())),
        Edit::RenameChat(id) => Command::Update(*id, ChatChange::Title(value.into())),
        Edit::DeleteChat(id) => Command::Delete(*id),
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
        Edit::RenameProject(_) | Edit::RenameChat(_) => DialogForm {
            title: if matches!(edit, Edit::RenameProject(_)) {
                "Rename project"
            } else {
                "Rename chat"
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
                .find(|p| p.id == *id)
                .map(|p| p.path.as_str()),
            detail: None,
            field: None,
            action: "Remove project",
            variant: ButtonVariant::Danger,
        },
        Edit::DeleteChat(id) => DialogForm {
            title: "Delete chat?",
            subject: Some(s.title(*id)),
            detail: Some("This also deletes its subagents. This can’t be undone."),
            field: None,
            action: "Delete",
            variant: ButtonVariant::Danger,
        },
    };
    form.build(
        |s: &mut Desktop| &mut s.navigation.value,
        |s| s.navigation.edit = None,
        edit_submit,
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

/// A top-level chat's actions. The orchestrator has none and never gets a
/// row menu.
pub(super) fn chat_menu(
    s: &Desktop,
    chat: &ConversationSummary,
    key: &str,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let (mut menu, context) = menu(s, key, t);
    let Some(context) = context else {
        return menu.build(&t.overlay, layer);
    };
    let id = chat.id;
    let pinned = chat.pinned;
    menu = menu.item(
        MenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_select_tx(
            move |s: &mut Desktop, tx| {
                s.navigation.menu = None;
                if s.connected() {
                    s.send(Command::Update(id, ChatChange::Pin(!pinned)), tx);
                }
            },
        ),
    );
    let archived = chat.archived;
    menu = menu
        .item(
            MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                let title = s
                    .summary(id)
                    .map_or_else(String::new, |chat| chat.title.clone());
                open_edit(s, Edit::RenameChat(id), &title, tx);
            }),
        )
        .item(
            MenuItem::new(if archived { "Unarchive" } else { "Archive" }).on_select_tx(
                move |s: &mut Desktop, tx| {
                    s.navigation.menu = None;
                    s.send(Command::Update(id, ChatChange::Archive(!archived)), tx);
                },
            ),
        );
    if chat.kind == ConversationKind::Chat && (!s.projects.is_empty() || chat.project.is_some()) {
        // Its destinations open beside it; pressing it again closes them.
        let destinations = std::iter::once(destination(chat, None))
            .chain(
                s.projects
                    .iter()
                    .map(|project| destination(chat, Some(project))),
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
            .on_select_tx(move |s: &mut Desktop, tx| delete_chat(s, id, tx)),
    );
    menu.build(&t.overlay, layer)
}

fn destination(chat: &ConversationSummary, project: Option<&Project>) -> MenuItem {
    let id = chat.id;
    let current = chat.project.as_deref() == project.map(|project| project.id.as_str());
    let project_id = project.map(|project| project.id.clone());
    MenuItem::new(project.map_or("No project", |project| project.name.as_str()))
        .checked(current)
        .disabled(current)
        .on_select_tx(move |s: &mut Desktop, tx| {
            s.navigation.menu = None;
            if let Some(project) = &project_id {
                s.navigation.expand_project(project);
            }
            s.send(
                Command::Update(id, ChatChange::Project(project_id.clone())),
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
        let new = project.id.clone();
        let rename = project.id.clone();
        let remove = project.id.clone();
        let name = project.name.clone();
        menu = menu
            .item(
                MenuItem::new("New chat")
                    .disabled(s.new_chat.is_some())
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
