//! Sidebar triage helpers: optimistic lifecycle mutations with 5-second undo
//! toasts, drag-and-drop commit, inline rename, project filter keyboard nav,
//! and confirmation/rename dialog submission.

use viz_client::protocol::{Position, ThreadId, TriageAction};

use super::{
    NavKey,
    selection::{apply_thread_change, prepare_draft},
    sidebar::{INLINE_RENAME_INPUT, close_project_filter},
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ProjectChange, ServerEffect, ThreadChange},
    model::{
        DragShelf, InlineRename, Model, NewThread, OptimisticUndo, ProjectFilter, SidebarEdit,
        SidebarOutline, UndoTriage, project_filter_options, thread_title,
    },
};

/// Auto-dismiss timeout for triage undo toasts (seconds).
const UNDO_TOAST_SECS: f64 = 5.0;

pub(super) fn start_filtered_draft(model: &mut Model, effects: &mut Vec<Effect>) {
    let seeded = match &model.sidebar.project_filter {
        ProjectFilter::Project(path) => model.cache().project(path).cloned(),
        ProjectFilter::NoProject => None,
        ProjectFilter::All => model.project().cloned(),
    };
    let draft = NewThread::new(model.selection.next_draft, seeded.as_ref());
    prepare_draft(model, draft, effects);
}

pub(super) fn handle_filter_key(
    model: &mut Model,
    key: NavKey,
    now: i64,
    effects: &mut Vec<Effect>,
) {
    let options = project_filter_options(model, now);
    match key {
        NavKey::Escape => close_project_filter(model, effects),
        NavKey::Down if !options.is_empty() => {
            model.sidebar.filter_active = (model.sidebar.filter_active + 1) % options.len();
        }
        NavKey::Up if !options.is_empty() => {
            model.sidebar.filter_active = model
                .sidebar
                .filter_active
                .checked_sub(1)
                .unwrap_or(options.len() - 1);
        }
        NavKey::Enter => {
            let idx = model
                .sidebar
                .filter_active
                .min(options.len().saturating_sub(1));
            if let Some(chosen) = options.get(idx) {
                model.sidebar.project_filter = chosen.to_filter();
                close_project_filter(model, effects);
            }
        }
        NavKey::Up | NavKey::Down | NavKey::Tab => {}
    }
}

pub(super) fn apply_triage(
    model: &mut Model,
    thread: ThreadId,
    action: TriageAction,
    undo_title: Option<&'static str>,
    now: i64,
    effects: &mut Vec<Effect>,
) {
    model.sidebar.menu = None;
    model.sidebar.snooze_popover = None;
    model.sidebar.hover_card = None;
    if !model.connected() {
        return;
    }
    let undo_info = undo_title.and_then(|title| {
        let summary = model.summary(thread)?;
        Some((
            title,
            thread_title(summary).to_owned(),
            UndoTriage {
                thread,
                prior: summary.lifecycle.clone(),
            },
        ))
    });
    apply_thread_change(model, thread, ThreadChange::Triage(action), now, effects);
    if let Some((title, detail, undo)) = undo_info {
        effects.push(Effect::UndoToast {
            title: title.to_owned(),
            detail: Some(detail),
            undo,
            secs: UNDO_TOAST_SECS,
        });
    }
}

pub(super) fn start_inline_rename(model: &mut Model, thread: ThreadId, effects: &mut Vec<Effect>) {
    if !model.connected() {
        return;
    }
    let Some(title) = model.summary(thread).map(|t| t.title.clone()) else {
        return;
    };
    model.sidebar.menu = None;
    model.sidebar.hover_card = None;
    model.sidebar.snooze_popover = None;
    model.sidebar.inline_rename = Some(InlineRename {
        thread,
        value: title.clone(),
    });
    effects.push(Effect::SyncEdit(EditSync::Replace {
        target: EditTarget::InlineRename,
        text: title,
        select_all: true,
    }));
    effects.push(Effect::Focus {
        key: INLINE_RENAME_INPUT,
        select_all: false,
        when_shown: true,
    });
}

pub(super) fn commit_inline_rename(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some(rename) = model.sidebar.inline_rename.take() else {
        return;
    };
    let trimmed = rename.value.trim().to_owned();
    if !trimmed.is_empty()
        && model.connected()
        && model
            .summary(rename.thread)
            .is_some_and(|summary| summary.title != trimmed)
    {
        apply_thread_change(
            model,
            rename.thread,
            ThreadChange::Title(trimmed),
            model.disclosure.tool_clock,
            effects,
        );
    }
}

pub(super) fn finish_drag(model: &mut Model, cancelled: bool, now: i64, effects: &mut Vec<Effect>) {
    let Some(drag) = model.sidebar.drag.take() else {
        return;
    };
    if cancelled || !drag.active || !model.connected() {
        return;
    }
    if drag.source_shelf == drag.target_shelf && drag.source_index == drag.target_index {
        return;
    }
    let outline = SidebarOutline::derive(model, now);
    let target_list: Vec<ThreadId> = match drag.target_shelf {
        DragShelf::Pinned => outline
            .pinned
            .iter()
            .filter(|t| t.id != drag.thread)
            .map(|t| t.id)
            .collect(),
        DragShelf::Active => outline
            .active
            .iter()
            .filter(|t| t.id != drag.thread)
            .map(|t| t.id)
            .collect(),
    };
    let adjusted_index =
        if drag.source_shelf == drag.target_shelf && drag.target_index > drag.source_index {
            drag.target_index.saturating_sub(1)
        } else {
            drag.target_index
        };
    let position = if adjusted_index == 0 {
        Position::Top
    } else if let Some(&before) = target_list.get(adjusted_index) {
        Position::Before { thread: before }
    } else {
        Position::Bottom
    };
    let (action, undo_title) = match (drag.source_shelf, drag.target_shelf) {
        (_, DragShelf::Pinned) => (TriageAction::Pin { position }, None),
        (DragShelf::Pinned, DragShelf::Active) if adjusted_index == 0 => {
            (TriageAction::Unpin, Some("Thread unpinned"))
        }
        (DragShelf::Pinned, DragShelf::Active) => {
            (TriageAction::Arrange { position }, Some("Thread unpinned"))
        }
        (DragShelf::Active, DragShelf::Active) => (TriageAction::Arrange { position }, None),
    };
    apply_triage(model, drag.thread, action, undo_title, now, effects);
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
    model.sidebar.hover_card = None;
    model.sidebar.snooze_popover = None;
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

pub(super) fn submit_edit(model: &mut Model, effects: &mut Vec<Effect>) {
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
