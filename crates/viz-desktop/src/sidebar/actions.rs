use rune::{El, PointerPress, Tx};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement};
use viz_client::protocol::{ConversationId, ConversationKind, ConversationSummary, Project};

use crate::{
    app::Desktop,
    backend::{ChatChange, Command, ProjectChange},
    theme::Theme,
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

    pub(super) fn at(key: &str, point: PointerPress) -> Self {
        Self {
            point: Some((point.x, point.y)),
            ..Self::new(key)
        }
    }
}

#[derive(Debug)]
pub(super) enum Edit {
    AddProject,
    RenameProject(String),
    RemoveProject(String),
    RenameChat(ConversationId),
}

fn open_edit(s: &mut Desktop, edit: Edit, value: &str, tx: &mut Tx) {
    s.navigation.menu = None;
    s.navigation.edit = Some(edit);
    s.navigation.value.replace_text(value);
    s.navigation.value.set_selection(0, value.len());
    tx.focus("edit-value", false);
}

pub(super) fn add_project(s: &mut Desktop, tx: &mut Tx) {
    open_edit(s, Edit::AddProject, "", tx);
}

fn edit_submit(s: &mut Desktop, tx: &mut Tx) {
    let Some(edit) = s.navigation.edit.as_ref() else {
        return;
    };
    let value = s.navigation.value.text.trim();
    if value.is_empty() && !matches!(edit, Edit::RemoveProject(_)) {
        return;
    }
    let command = match edit {
        Edit::AddProject => Command::Project(ProjectChange::Add(value.into())),
        Edit::RenameProject(id) => Command::Project(ProjectChange::Rename {
            id: id.clone(),
            name: value.into(),
        }),
        Edit::RemoveProject(id) => Command::Project(ProjectChange::Remove(id.clone())),
        Edit::RenameChat(id) => Command::Update(*id, ChatChange::Title(value.into())),
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
        Edit::AddProject => DialogForm {
            title: "Add project",
            detail: None,
            field: Some((&s.navigation.value, "Directory path")),
            action: "Add project",
            variant: ButtonVariant::Submit,
        },
        Edit::RenameProject(_) | Edit::RenameChat(_) => DialogForm {
            title: if matches!(edit, Edit::RenameProject(_)) {
                "Rename project"
            } else {
                "Rename chat"
            },
            detail: None,
            field,
            action: "Save",
            variant: ButtonVariant::Submit,
        },
        Edit::RemoveProject(id) => DialogForm {
            title: "Remove project",
            detail: s
                .projects
                .iter()
                .find(|p| p.id == *id)
                .map(|p| p.path.as_str()),
            field: None,
            action: "Remove project",
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

fn menu<'a>(s: &'a Desktop, key: &str, selected: bool, t: &Theme) -> (Menu, Option<&'a Context>) {
    let context = s
        .navigation
        .menu
        .as_ref()
        .filter(|context| context.key == key && s.connected());
    let target = key.to_owned();
    let trigger = ui::icon_button(&ui::MORE, "Actions", IconButtonVariant::GhostMuted)
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
        .key(format!("{key}-actions"))
        .size(20.0, 23.0)
        .group_opacity(
            if selected || context.is_some() {
                1.0
            } else {
                0.0
            },
            1.0,
        );
    let mut menu = Menu::new(format!("{key}-context"))
        .trigger(trigger)
        .standard()
        .placement(Placement::BottomEnd)
        .min_width(158.0)
        .open(context.is_some())
        .on_close(|s: &mut Desktop| s.navigation.menu = None);
    if let Some((x, y)) = context.and_then(|context| context.point) {
        menu = menu.at(x, y);
    }
    (menu, context)
}

pub(super) fn chat_menu(
    s: &Desktop,
    chat: &ConversationSummary,
    key: &str,
    selected: bool,
    t: &Theme,
    layer: &mut Vec<El>,
) -> El {
    let (mut menu, context) = menu(s, key, selected, t);
    let Some(context) = context else {
        return menu.build(&t.k, layer);
    };
    let id = chat.id;
    if context.moving {
        menu = menu
            .item(MenuItem::new("Back").on_select(|s: &mut Desktop| {
                if let Some(context) = &mut s.navigation.menu {
                    context.moving = false;
                }
            }))
            .separator()
            .group_label("Move to project");
        menu = menu.item(destination(chat, None));
        menu = ui::menu_items(
            menu,
            s.projects
                .iter()
                .map(|project| destination(chat, Some(project))),
        );
    } else {
        let archived = chat.archived;
        menu = menu
            .item(
                MenuItem::new("Rename").on_select_tx(move |s: &mut Desktop, tx| {
                    let title = s
                        .summary(id)
                        .and_then(|chat| chat.title.clone())
                        .unwrap_or_default();
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
        if chat.kind == ConversationKind::Chat && (!s.projects.is_empty() || chat.project.is_some())
        {
            menu = menu.item(
                MenuItem::new("Move to project").on_select(|s: &mut Desktop| {
                    if let Some(context) = &mut s.navigation.menu {
                        context.moving = true;
                    }
                }),
            );
        }
    }
    menu.build(&t.k, layer)
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
    let (mut menu, context) = menu(s, key, false, t);
    if context.is_some() {
        let (new, rename, remove) = (project.id.clone(), project.id.clone(), project.id.clone());
        let name = project.name.clone();
        menu = menu
            .item(
                MenuItem::new("New chat")
                    .disabled(s.new_chat.is_some())
                    .on_select_tx(move |s: &mut Desktop, tx| {
                        s.navigation.menu = None;
                        s.create(Some(new.clone()), tx);
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
    menu.build(&t.k, layer)
}
