//! Pure transitions for the unified triage sidebar: shelves, project filter
//! combobox, hover cards, inline rename, drag-and-drop, and routing into
//! [`super::sidebar_triage`].

use std::collections::BTreeMap;

use viz_client::protocol::{ThreadId, TriageAction, WorktreeTarget};

pub(super) use super::sidebar_triage::open_edit;
use super::{
    NavKey,
    selection::{clear_selection, prepare_draft, select_thread},
    sidebar_triage::{
        apply_triage, commit_inline_rename, finish_drag, handle_filter_key, start_filtered_draft,
        start_inline_rename, submit_edit,
    },
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect, ThreadChange},
    model::{
        ContextMenu, ContextSubmenu, DragShelf, Model, NewThread, ProjectFilter, SETTLED_PAGE_SIZE,
        SidebarDrag, SidebarEdit, SidebarOutline, SnoozePreset, UndoTriage, nth_visible_thread,
    },
};

/// Element key of the project filter combobox search input.
pub const PROJECT_FILTER_INPUT: &str = "project-filter-input";
/// Element key of the inline thread title rename input.
pub const INLINE_RENAME_INPUT: &str = "sidebar-inline-rename";

/// Messages emitted by the sidebar view, overlays, drag gestures, and
/// shortcuts.
#[derive(Debug)]
pub enum SidebarMsg {
    /// Start a new draft thread seeded from the active project filter or
    /// current thread's project.
    NewThread,
    /// Start a new draft thread seeded with an explicit project and optional
    /// branch.
    NewThreadIn {
        /// Target project path, or `None` for No project.
        project: Option<String>,
        /// Optional branch name to base a new worktree on when non-default.
        branch: Option<String>,
    },
    /// Discard the active unsent draft thread.
    DiscardDraft,
    /// Open or close the Project Filter Combobox.
    ToggleProjectFilter,
    /// Close the Project Filter Combobox.
    CloseProjectFilter,
    /// Project filter search text edited.
    ProjectFilterQueryChanged(String),
    /// Keyboard navigation key inside the Project Filter Combobox.
    ProjectFilterKey {
        /// Navigation key.
        key: NavKey,
        /// Current timestamp in Unix ms for counting threads.
        now: i64,
    },
    /// Apply a project filter (`All`, one project path, or `NoProject`).
    SetProjectFilter(ProjectFilter),
    /// Toggle a collapsible triage shelf (`working`, `snoozed`, `settled`).
    ToggleShelf(&'static str),
    /// Reveal the next page of settled threads in the Settled drawer.
    LoadMoreSettled,
    /// Track the thread row currently under the pointer so hover quick actions
    /// swap in without reserving horizontal space on unhovered rows.
    HoverRow(Option<ThreadId>),
    /// Show or hide the delayed right-anchored hover preview card.
    HoverCard(Option<ThreadId>),
    /// Open or close the hover quick-action snooze preset popover for `thread`.
    ToggleSnoozePopover(ThreadId),
    /// Close any open snooze preset popover.
    CloseSnoozePopover,
    /// Open or close the footer environment switcher menu.
    ToggleEnvMenu,
    /// Close the footer environment switcher menu.
    CloseEnvMenu,
    /// Open or close the footer workspace/shortcuts info popover.
    ToggleInfoMenu,
    /// Close the footer workspace/shortcuts info popover.
    CloseInfoMenu,
    /// Open the thread context menu at pointer coordinates `(x, y)`.
    OpenRowMenuAt {
        /// Row element key (`thread-<id>`).
        key: String,
        /// Horizontal pointer coordinate.
        x: f32,
        /// Vertical pointer coordinate.
        y: f32,
    },
    /// Close any open row context menu.
    CloseRowMenu,
    /// Expand or collapse a submenu inside the open thread context menu.
    ToggleSubmenu(ContextSubmenu),
    /// Set the active hover-expanded submenu inside the open thread context
    /// menu.
    SetSubmenu(Option<ContextSubmenu>),
    /// Apply a lifecycle triage action optimistically with optional 5s undo
    /// toast.
    Triage {
        /// Target thread id.
        thread: ThreadId,
        /// Protocol triage action.
        action: TriageAction,
        /// When `Some`, emits a 5-second bottom undo toast with this title.
        undo_title: Option<&'static str>,
        /// Current wall-clock timestamp in Unix ms.
        now: i64,
    },
    /// Restore a prior lifecycle from a triage undo toast.
    UndoTriage(UndoTriage),
    /// Snooze `thread` using one of the standard presets.
    SnoozePreset {
        /// Target thread id.
        thread: ThreadId,
        /// Chosen preset.
        preset: SnoozePreset,
        /// Current wall-clock timestamp in Unix ms.
        now: i64,
    },
    /// Regenerate `thread`'s title from the harness or conversation.
    RetitleThread(ThreadId),
    /// Fork `thread` from its head and select the forked copy.
    ForkThread(ThreadId),
    /// Copy `value` to the clipboard and surface a short confirmation toast.
    CopyText {
        /// Human-readable label of what was copied.
        label: &'static str,
        /// Plain text copied to the clipboard.
        value: String,
    },
    /// Reassign a thread to another project or detach it.
    MoveThreadToProject {
        /// Target thread id.
        thread: ThreadId,
        /// Destination project path, or `None` for No project.
        project: Option<String>,
    },
    /// Begin inline title editing inside Row 2 of `thread`'s card.
    StartInlineRename(ThreadId),
    /// Inline rename field text changed.
    InlineRenameChanged(String),
    /// Commit the active inline title edit.
    CommitInlineRename,
    /// Cancel the active inline title edit.
    CancelInlineRename,
    /// Pointer pressed on a draggable Pinned or Active thread card.
    DragStart {
        /// Thread id.
        thread: ThreadId,
        /// Source shelf (`Pinned` or `Active`).
        shelf: DragShelf,
    },
    /// Pointer moved during a card drag gesture.
    DragMove {
        /// Vertical displacement from press point in points.
        dy: f32,
        /// Current pointer in window points.
        point: [f32; 2],
        /// Current timestamp in Unix ms.
        now: i64,
    },
    /// Pointer released or cancelled on a card drag gesture.
    DragEnd {
        /// True when cancelled (e.g., Escape or lost focus).
        cancelled: bool,
        /// Current timestamp in Unix ms.
        now: i64,
    },
    /// Open a modal rename or delete confirmation dialog.
    OpenEdit {
        /// Dialog variant.
        edit: SidebarEdit,
        /// Initial field text (empty for confirmations).
        initial_value: String,
    },
    /// Modal dialog field text edited.
    EditValueChanged(String),
    /// Dismiss the active sidebar modal dialog.
    CloseEdit,
    /// Submit the active sidebar modal dialog.
    SubmitEdit,
    /// Open the Add Project palette.
    OpenAddProject,
    /// Open delete confirmation for `thread` if present.
    OpenDeleteThread(ThreadId),
    /// Toggle sidebar open/closed (`Cmd/Ctrl+B`).
    TogglePane,
    /// Jump to the `index`-th (`0`-based) visible thread (`Cmd/Ctrl+1..9`).
    JumpToIndex {
        /// Zero-based position across visible primary shelves.
        index: usize,
        /// Current timestamp in Unix ms.
        now: i64,
    },
    /// Dismiss transient popovers, inline rename, or an active drag on Escape.
    DismissTransient,
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive reducer over sidebar messages"
)]
pub(super) fn update(model: &mut Model, msg: SidebarMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        SidebarMsg::NewThread => start_filtered_draft(model, &mut effects),
        SidebarMsg::NewThreadIn { project, branch } => {
            model.sidebar.menu = None;
            let resolved = project
                .as_deref()
                .and_then(|path| model.cache().project(path))
                .cloned();
            let mut draft = NewThread::new(model.selection.next_draft, resolved.as_ref());
            if let (Some(project), Some(branch)) = (&resolved, branch)
                && project.git
                && project.branch.as_deref() != Some(branch.as_str())
            {
                draft.worktree = Some(WorktreeTarget::New {
                    base: Some(branch),
                    branch: None,
                });
            }
            prepare_draft(model, draft, &mut effects);
        }
        SidebarMsg::DiscardDraft => {
            if model.selection.new_thread.take().is_some() {
                clear_selection(model, &mut effects);
                effects.push(Effect::SetWindowTitle(model.window_title()));
            }
        }
        SidebarMsg::ToggleProjectFilter => {
            if model.sidebar.filter_open {
                close_project_filter(model, &mut effects);
            } else {
                model.sidebar.dismiss_overlays();
                model.sidebar.filter_open = true;
                effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::ProjectFilter)));
                effects.push(Effect::Focus {
                    key: PROJECT_FILTER_INPUT,
                    select_all: false,
                    when_shown: true,
                });
            }
        }
        SidebarMsg::CloseProjectFilter => close_project_filter(model, &mut effects),
        SidebarMsg::ProjectFilterQueryChanged(query) => {
            model.sidebar.filter_query = query;
            model.sidebar.filter_active = 0;
        }
        SidebarMsg::ProjectFilterKey { key, now } => {
            handle_filter_key(model, key, now, &mut effects);
        }
        SidebarMsg::SetProjectFilter(filter) => {
            model.sidebar.project_filter = filter;
            model.sidebar.menu = None;
            close_project_filter(model, &mut effects);
        }
        SidebarMsg::ToggleShelf(key) => {
            if !model.sidebar.collapsed_shelves.remove(key) {
                model.sidebar.collapsed_shelves.insert(key.to_owned());
            }
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
        SidebarMsg::LoadMoreSettled => {
            model.sidebar.settled_limit = model
                .sidebar
                .settled_limit
                .saturating_add(SETTLED_PAGE_SIZE);
        }
        SidebarMsg::HoverRow(target) => {
            model.sidebar.hovered_thread = target;
        }
        SidebarMsg::HoverCard(target) => {
            let blocked = model.sidebar.menu.is_some()
                || model.sidebar.snooze_popover.is_some()
                || model.sidebar.filter_open
                || model.sidebar.inline_rename.is_some()
                || model.sidebar.drag.as_ref().is_some_and(|d| d.active);
            model.sidebar.hover_card = if blocked { None } else { target };
        }
        SidebarMsg::ToggleSnoozePopover(thread) => {
            model.sidebar.hover_card = None;
            model.sidebar.menu = None;
            model.sidebar.snooze_popover = if model.sidebar.snooze_popover == Some(thread) {
                None
            } else {
                Some(thread)
            };
        }
        SidebarMsg::CloseSnoozePopover => model.sidebar.snooze_popover = None,
        SidebarMsg::ToggleEnvMenu => {
            let open = !model.sidebar.env_menu;
            model.sidebar.info_menu = false;
            model.sidebar.env_menu = open;
        }
        SidebarMsg::CloseEnvMenu => model.sidebar.env_menu = false,
        SidebarMsg::ToggleInfoMenu => {
            let open = !model.sidebar.info_menu;
            model.sidebar.env_menu = false;
            model.sidebar.info_menu = open;
        }
        SidebarMsg::CloseInfoMenu => model.sidebar.info_menu = false,
        SidebarMsg::OpenRowMenuAt { key, x, y } => {
            if model.connected() {
                model.sidebar.hover_card = None;
                model.sidebar.snooze_popover = None;
                model.sidebar.menu = Some(ContextMenu::at(key, x, y));
            }
        }
        SidebarMsg::CloseRowMenu => model.sidebar.menu = None,
        SidebarMsg::ToggleSubmenu(sub) => {
            if let Some(menu) = &mut model.sidebar.menu {
                menu.submenu = if menu.submenu == Some(sub) {
                    None
                } else {
                    Some(sub)
                };
            }
        }
        SidebarMsg::SetSubmenu(sub) => {
            if let Some(menu) = &mut model.sidebar.menu
                && menu.submenu != sub
            {
                menu.submenu = sub;
            }
        }
        SidebarMsg::Triage {
            thread,
            action,
            undo_title,
            now,
        } => apply_triage(model, thread, action, undo_title, now, &mut effects),
        SidebarMsg::UndoTriage(undo) => {
            if model.connected() {
                super::selection::apply_thread_change(
                    model,
                    undo.thread,
                    ThreadChange::Triage(TriageAction::Restore {
                        lifecycle: undo.prior,
                    }),
                    model.disclosure.tool_clock,
                    &mut effects,
                );
            }
        }
        SidebarMsg::SnoozePreset {
            thread,
            preset,
            now,
        } => apply_triage(
            model,
            thread,
            TriageAction::Snooze {
                until: preset.until_ms(now),
            },
            Some("Thread snoozed"),
            now,
            &mut effects,
        ),
        SidebarMsg::RetitleThread(thread) => {
            model.sidebar.menu = None;
            if model.connected() {
                effects.push(Effect::Server(ServerEffect::Retitle(thread)));
            }
        }
        SidebarMsg::ForkThread(thread) => {
            model.sidebar.menu = None;
            if model.connected() {
                effects.push(Effect::Server(ServerEffect::fork_head(thread)));
            }
        }
        SidebarMsg::CopyText { label, value } => {
            model.sidebar.menu = None;
            effects.push(Effect::CopyText(value));
            effects.push(Effect::Toast {
                title: format!("{label} copied"),
                detail: None,
            });
        }
        SidebarMsg::MoveThreadToProject { thread, project } => {
            model.sidebar.menu = None;
            super::selection::apply_thread_change(
                model,
                thread,
                ThreadChange::Project(project),
                model.disclosure.tool_clock,
                &mut effects,
            );
        }
        SidebarMsg::StartInlineRename(thread) => start_inline_rename(model, thread, &mut effects),
        SidebarMsg::InlineRenameChanged(value) => {
            if let Some(rename) = &mut model.sidebar.inline_rename {
                rename.value = value;
            }
        }
        SidebarMsg::CommitInlineRename => commit_inline_rename(model, &mut effects),
        SidebarMsg::CancelInlineRename => model.sidebar.inline_rename = None,
        SidebarMsg::DragStart { thread, shelf } => {
            if model.connected() {
                model.sidebar.hover_card = None;
                model.sidebar.drag = Some(SidebarDrag {
                    thread,
                    source_shelf: shelf,
                    target: None,
                    dy: 0.0,
                    active: false,
                    rects: BTreeMap::new(),
                });
            }
        }
        SidebarMsg::DragMove { dy, point, now } => {
            if let Some(mut drag) = model.sidebar.drag.take() {
                let outline = SidebarOutline::derive(model, now);
                drag.move_to(dy, point, &outline);
                if drag.active {
                    model.sidebar.hover_card = None;
                    model.sidebar.snooze_popover = None;
                }
                model.sidebar.drag = Some(drag);
            }
        }
        SidebarMsg::DragEnd { cancelled, now } => finish_drag(model, cancelled, now, &mut effects),
        SidebarMsg::OpenEdit {
            edit,
            initial_value,
        } => open_edit(model, edit, initial_value, &mut effects),
        SidebarMsg::EditValueChanged(value) => model.sidebar.edit_value = value,
        SidebarMsg::CloseEdit => model.sidebar.edit = None,
        SidebarMsg::SubmitEdit => submit_edit(model, &mut effects),
        SidebarMsg::OpenAddProject => effects.extend(super::palette::update(
            model,
            super::PaletteMsg::OpenProject(crate::model::projects::ProjectView::Sources),
        )),
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
        SidebarMsg::JumpToIndex { index, now } => {
            if let Some(id) = nth_visible_thread(model, index, now) {
                select_thread(model, id, &mut effects);
            }
        }
        SidebarMsg::DismissTransient => {
            model.sidebar.inline_rename = None;
            model.sidebar.drag = None;
            model.sidebar.hover_card = None;
            model.sidebar.snooze_popover = None;
            model.sidebar.info_menu = false;
            if model.sidebar.filter_open {
                close_project_filter(model, &mut effects);
            }
        }
    }
    effects
}

pub(super) fn close_project_filter(model: &mut Model, effects: &mut Vec<Effect>) {
    model.sidebar.filter_open = false;
    model.sidebar.filter_query.clear();
    model.sidebar.filter_active = 0;
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::ProjectFilter)));
}
