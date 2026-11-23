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
    effects::{EditSync, EditTarget, Effect, ServerEffect, ThreadChange},
    model::{
        DragShelf, InlineRename, Model, NewThread, ProjectFilter, SidebarEdit, SidebarOutline,
        UndoTriage, project_filter_options, thread_title,
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
        NavKey::Down | NavKey::Up if !options.is_empty() => {
            let count = options.len();
            let current = model.sidebar.filter_active.min(count - 1);
            let next = if key == NavKey::Up {
                (current + count - 1) % count
            } else {
                (current + 1) % count
            };
            let item = options[next].key();
            model.sidebar.filter_active = next;
            effects.push(Effect::ScrollIntoView {
                list: "project-filter-combobox-scroll",
                item,
            });
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
    let Some((target_shelf, before)) = drag.target else {
        return;
    };
    let outline = SidebarOutline::derive(model, now);
    let rows = |shelf| match shelf {
        DragShelf::Pinned => &outline.pinned,
        DragShelf::Active => &outline.active,
    };
    let Some(source_index) = rows(drag.source_shelf)
        .iter()
        .position(|thread| thread.id == drag.thread)
    else {
        return;
    };
    if drag.source_shelf == target_shelf
        && rows(target_shelf)
            .get(source_index + 1)
            .map(|thread| thread.id)
            == before
    {
        return;
    }
    let target_rows = rows(target_shelf);
    let position = if let Some(thread) = before {
        if !target_rows.iter().any(|row| row.id == thread) {
            return;
        }
        Position::Before { thread }
    } else {
        Position::Bottom
    };
    let (action, undo_title) = match (drag.source_shelf, target_shelf) {
        (_, DragShelf::Pinned) => (TriageAction::Pin { position }, None),
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
    model.sidebar.edit = None;
    match edit {
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
