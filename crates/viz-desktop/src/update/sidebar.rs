//! Pure transitions for the sidebar: grouping mode, filter, collapses,
//! context menus, rename/delete dialogs, and optimistic project/thread edits.

use viz_client::protocol::ThreadId;

use super::selection::{apply_thread_change, select_thread};
use crate::{
    effects::{EditSync, EditTarget, Effect, ProjectChange, ServerEffect, ThreadChange},
    model::{ContextMenu, Model, OptimisticUndo, SidebarEdit, SidebarMode, sidebar_first_match},
};

/// Messages emitted by the sidebar view, its dialogs, and shortcuts.
#[derive(Debug)]
pub enum SidebarMsg {
    /// Switch between Recent and Projects modes.
    SetMode(SidebarMode),
    /// Filter query text edited.
    FilterChanged(String),
    /// Submit active sidebar filter to open the first matching thread.
    FilterSubmit,
    /// Expand or collapse a project folder in Projects mode.
    ToggleProject(String),
    /// Expand or collapse a parent thread's delegated children.
    ToggleParent(ThreadId),
    /// Open or close the footer environment menu.
    ToggleEnvMenu,
    /// Close the footer environment menu.
    CloseEnvMenu,
    /// Toggle the row actions menu anchored to `key`.
    ToggleRowMenu(String),
    /// Open the row context menu at pointer coordinates `(x, y)`.
    OpenRowMenuAt {
        /// Row element key.
        key: String,
        /// Horizontal pointer coordinate.
        x: f32,
        /// Vertical pointer coordinate.
        y: f32,
    },
    /// Close any open row context menu.
    CloseRowMenu,
    /// Toggle the "Move to project" submenu inside the open thread menu.
    ToggleMoveSubmenu,
    /// Open a rename or delete confirmation dialog.
    OpenEdit {
        /// Dialog variant.
        edit: SidebarEdit,
        /// Initial field text (empty for confirmations).
        initial_value: String,
    },
    /// Rename dialog field text edited.
    EditValueChanged(String),
    /// Dismiss the active sidebar modal dialog.
    CloseEdit,
    /// Submit the active sidebar modal dialog.
    SubmitEdit,
    /// Open the OS folder picker to register a project.
    OpenAddProject,
    /// User picked a folder path to register as a project.
    ProjectFolderPicked(String),
    /// Opening the OS folder picker failed.
    ProjectFolderFailed(String),
    /// Pin or unpin a thread (optimistic with rollback).
    PinThread {
        /// Target thread id.
        thread: ThreadId,
        /// Desired pinned state.
        pinned: bool,
    },
    /// Archive or unarchive a thread (optimistic with rollback).
    ArchiveThread {
        /// Target thread id.
        thread: ThreadId,
        /// Desired archived state.
        archived: bool,
    },
    /// Move a thread to another project or detach it (optimistic with
    /// rollback).
    MoveThreadToProject {
        /// Target thread id.
        thread: ThreadId,
        /// Destination project path, or `None` for No project.
        project: Option<String>,
    },
    /// Open delete confirmation for `thread` if present.
    OpenDeleteThread(ThreadId),
    /// Toggle sidebar open/closed (keyboard shortcut or palette).
    TogglePane,
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive handler for sidebar view and dialog messages"
)]
pub(super) fn update(model: &mut Model, msg: SidebarMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        SidebarMsg::SetMode(mode) => {
            if model.sidebar.mode != mode {
                model.sidebar.mode = mode;
                effects.push(Effect::SavePrefs(model.to_prefs()));
            }
        }
        SidebarMsg::FilterChanged(query) => {
            model.sidebar.filter = query;
        }
        SidebarMsg::FilterSubmit => {
            if let Some(id) = sidebar_first_match(model) {
                model.sidebar.filter.clear();
                effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::SidebarFilter)));
                select_thread(model, id, &mut effects);
            }
        }
        SidebarMsg::ToggleProject(path) => {
            if !model.sidebar.collapsed_projects.remove(&path) {
                model.sidebar.collapsed_projects.insert(path);
            }
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
        SidebarMsg::ToggleParent(id) => {
            if !model.sidebar.collapsed_parents.remove(&id) {
                model.sidebar.collapsed_parents.insert(id);
            }
        }
        SidebarMsg::ToggleEnvMenu => {
            model.sidebar.env_menu = !model.sidebar.env_menu;
        }
        SidebarMsg::CloseEnvMenu => {
            model.sidebar.env_menu = false;
        }
        SidebarMsg::ToggleRowMenu(key) => {
            model.sidebar.menu = if model
                .sidebar
                .menu
                .as_ref()
                .is_some_and(|ctx| ctx.key == key)
            {
                None
            } else {
                Some(ContextMenu::anchored(key))
            };
        }
        SidebarMsg::OpenRowMenuAt { key, x, y } => {
            if model.connected() {
                model.sidebar.menu = Some(ContextMenu::at(key, x, y));
            }
        }
        SidebarMsg::CloseRowMenu => {
            model.sidebar.menu = None;
        }
        SidebarMsg::ToggleMoveSubmenu => {
            if let Some(menu) = &mut model.sidebar.menu {
                menu.moving = !menu.moving;
            }
        }
        SidebarMsg::OpenEdit {
            edit,
            initial_value,
        } => open_edit(model, edit, initial_value, &mut effects),
        SidebarMsg::EditValueChanged(value) => {
            model.sidebar.edit_value = value;
        }
        SidebarMsg::CloseEdit => {
            model.sidebar.edit = None;
        }
        SidebarMsg::SubmitEdit => submit_edit(model, &mut effects),
        SidebarMsg::OpenAddProject => {
            if model.connected() {
                model.sidebar.menu = None;
                effects.push(Effect::PickProjectFolder);
            }
        }
        SidebarMsg::ProjectFolderPicked(path) => {
            effects.push(Effect::Server(ServerEffect::Project {
                op_id: None,
                change: ProjectChange::Add(path),
            }));
        }
        SidebarMsg::ProjectFolderFailed(error) => {
            effects.push(Effect::Toast {
                title: "Couldn’t open the folder picker".into(),
                detail: Some(error),
            });
        }
        SidebarMsg::PinThread { thread, pinned } => {
            model.sidebar.menu = None;
            if model.connected() {
                apply_thread_change(model, thread, ThreadChange::Pin(pinned), &mut effects);
            }
        }
        SidebarMsg::ArchiveThread { thread, archived } => {
            model.sidebar.menu = None;
            if model.connected() {
                apply_thread_change(model, thread, ThreadChange::Archive(archived), &mut effects);
            }
        }
        SidebarMsg::MoveThreadToProject { thread, project } => {
            model.sidebar.menu = None;
            if let Some(path) = &project
                && model.sidebar.collapsed_projects.remove(path)
            {
                effects.push(Effect::SavePrefs(model.to_prefs()));
            }
            apply_thread_change(model, thread, ThreadChange::Project(project), &mut effects);
        }
        SidebarMsg::OpenDeleteThread(id) => {
            if model.summary(id).is_some() {
                open_edit(
                    model,
                    SidebarEdit::DeleteThread(id),
                    String::new(),
                    &mut effects,
                );
            }
        }
        SidebarMsg::TogglePane => {
            model.panels.sidebar.open = !model.panels.sidebar.open;
            effects.push(Effect::SyncSidebarPane {
                open: Some(model.panels.sidebar.open),
                width: None,
            });
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
    }
    effects
}

pub(super) fn open_edit(
    model: &mut Model,
    edit: SidebarEdit,
    initial_value: String,
    effects: &mut Vec<Effect>,
) {
    if !model.connected() {
        return;
    }
    model.sidebar.menu = None;
    model.sidebar.edit = Some(edit);
    model.sidebar.edit_value.clone_from(&initial_value);
    effects.push(Effect::SyncEdit(EditSync::Replace {
        target: EditTarget::SidebarDialog,
        text: initial_value,
        select_all: true,
    }));
    effects.push(Effect::Focus {
        key: "edit-value",
        select_all: false,
        when_shown: true,
    });
}

fn submit_edit(model: &mut Model, effects: &mut Vec<Effect>) {
    if !model.connected() {
        return;
    }
    let Some(edit) = model.sidebar.edit.clone() else {
        return;
    };
    let value = model.sidebar.edit_value.trim().to_owned();
    if value.is_empty()
        && !matches!(
            edit,
            SidebarEdit::RemoveProject(_)
                | SidebarEdit::DeleteThread(_)
                | SidebarEdit::DeleteWithWorktree(_)
        )
    {
        return;
    }
    model.sidebar.edit = None;
    let env = model.active_env_key().to_owned();
    match edit {
        SidebarEdit::RenameProject(id) => {
            let prior_name = model.cache_mut().project_mut(&id).map(|project| {
                let prior = project.name.clone();
                project.name.clone_from(&value);
                prior
            });
            let op_id = prior_name.map(|prior_name| {
                model.optimistic.record(OptimisticUndo::ProjectRename {
                    env,
                    path: id.clone(),
                    prior_name,
                })
            });
            effects.push(Effect::Server(ServerEffect::Project {
                op_id,
                change: ProjectChange::Rename { id, name: value },
            }));
        }
        SidebarEdit::RemoveProject(id) => {
            let op_id = model
                .cache_mut()
                .remove_project(&id)
                .map(|(index, project)| {
                    model.optimistic.record(OptimisticUndo::ProjectRemove {
                        env,
                        index,
                        project: Box::new(project),
                    })
                });
            effects.push(Effect::Server(ServerEffect::Project {
                op_id,
                change: ProjectChange::Remove(id),
            }));
        }
        SidebarEdit::RenameThread(id) => {
            apply_thread_change(model, id, ThreadChange::Title(value), effects);
        }
        SidebarEdit::DeleteThread(thread) => {
            effects.push(Effect::Server(ServerEffect::Delete {
                thread,
                remove_worktree: false,
            }));
        }
        SidebarEdit::DeleteWithWorktree(thread) => {
            effects.push(Effect::Server(ServerEffect::Delete {
                thread,
                remove_worktree: true,
            }));
        }
    }
}
