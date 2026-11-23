//! Pure transitions for the composer: prompt text, atomic pills, attachments,
//! sending/steering/queueing, inline completions, Home pickers, model/thinking
//! picker, runner options, access mode, and tasks dock actions.

use viz_client::protocol::{RunId, SendMode};

pub use super::home::{
    DRAFT_BRANCH_PICKER, DRAFT_BRANCH_SEARCH, HOME_ENV_PICKER, HOME_PROJECT_PICKER,
    HOME_PROJECT_SEARCH,
};
use super::{
    NavKey,
    composer_actions::{
        cancel_edit_queued, choose_completion, edit_queued, handle_completion_key, poll_completion,
        send_input, step_history,
    },
    home::{
        close_home_project_picker, handle_draft_branch_key, handle_home_project_key,
        toggle_draft_branch_picker, toggle_home_project_picker, toggle_project_pin,
    },
};
use crate::{
    effects::{AttachmentSource, Effect, ServerEffect},
    model::{DraftCursor, Model, NewThread},
};

/// Guard interval in seconds preventing accidental double-click re-send right
/// after stopping a run.
const STOP_GUARD_SECS: f64 = 0.5;

/// Messages emitted by the composer, Home pickers, completion drawer, queued
/// stack, and tasks dock.
#[derive(Debug)]
pub enum ComposerMsg {
    /// Prompt text edited by the user.
    DraftChanged(String),
    /// Submit the prompt (launch draft, update queued edit, or send/steer
    /// thread).
    SendInput,
    /// Queue the typed prompt while the active thread is running
    /// (`Ctrl/Cmd+Enter`).
    QueueInput,
    /// Abort the active turn on the selected thread.
    Abort,
    /// Abort the active turn while arming the 500ms post-stop click guard.
    AbortAt(f64),
    /// Start loading attachments from file picker, drag-drop, or clipboard.
    StartAttachments(AttachmentSource),
    /// Remove an attached file by index.
    RemoveAttachment(usize),
    /// Retry a failed attachment slot (clears its error status).
    /// File drag entered or left the composer card.
    SetDragOver(bool),
    /// Open or close the full-size attachment image lightbox.
    OpenLightbox(Option<usize>),
    /// Toggle compact-before-send when context usage is high.
    ToggleCompactBeforeSend,
    /// Step backward (`up = true`) or forward through thread prompt history.
    HistoryStep {
        /// True for `ArrowUp`, false for `ArrowDown`.
        up: bool,
    },
    /// Promote a queued message to steer immediately.
    SteerQueued(RunId),
    /// Load a queued message into the composer for editing.
    EditQueued(RunId),
    /// Cancel editing a queued message and restore the previous draft.
    CancelEditQueued,
    /// Remove a queued message from the queue.
    RemoveQueued(RunId),
    /// Expand or collapse queued messages beyond the first three.
    ToggleQueuedExpanded,
    /// Periodic completion poll syncing the caret and firing debounced search.
    PollCompletion {
        /// Current UI clock in seconds.
        clock: f64,
        /// Pure snapshot of the composer editor focus/caret.
        cursor: DraftCursor,
    },
    /// Pointer hovered completion row `index`.
    CompletionHover(usize),
    /// Navigation key pressed while completion drawer is active.
    CompletionKey {
        /// Navigation key.
        key: NavKey,
        /// Current composer cursor snapshot.
        cursor: DraftCursor,
    },
    /// User picked completion row `index`.
    ChooseCompletion {
        /// Row index in the active completion drawer.
        index: usize,
        /// Current composer cursor snapshot.
        cursor: DraftCursor,
    },
    /// Open or close the Home hero searchable Project Picker combobox.
    ToggleHomeProjectPicker,
    /// Close the Home hero Project Picker combobox.
    CloseHomeProjectPicker,
    /// Search text in the Home hero Project Picker changed.
    HomeProjectQueryChanged(String),
    /// Keyboard navigation inside the Home hero Project Picker.
    HomeProjectKey(NavKey),
    /// Pin or unpin a project directly from the Home hero Project Picker.
    ToggleProjectPin(String),
    /// Open or close the Home context strip branch picker and refresh refs.
    ToggleDraftBranchPicker(String),
    /// Search text in the Home context strip branch picker changed.
    DraftBranchQueryChanged(String),
    /// Keyboard navigation inside the Home context strip branch picker.
    DraftBranchKey(NavKey),
    /// Open or close an inline picker menu by key.
    TogglePicker(String),
    /// Open or close a checkout picker, loading branches/worktrees on open.
    ToggleCheckoutPicker {
        /// Picker key (`draft-checkout` or `thread-checkout`).
        key: String,
        /// Project path whose branches and worktrees should load.
        project: String,
    },
    /// Close whichever inline picker is open.
    ClosePicker,
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive handler for composer, completion, queued, and picker messages"
)]
pub(super) fn update(model: &mut Model, msg: ComposerMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        ComposerMsg::DraftChanged(text) => {
            model.composer.draft.text = text;
            model.composer.history_cursor = None;
            model.composer.history_stash = None;
            if model.selection.selected.is_none()
                && model.selection.new_thread.is_none()
                && model.connected()
                && !model.composer.draft.text.is_empty()
            {
                let draft = NewThread::new(model.selection.next_draft, None);
                model.selection.next_draft += 1;
                model.selection.new_thread = Some(draft);
            }
        }
        ComposerMsg::SendInput => send_input(model, None, &mut effects),
        ComposerMsg::QueueInput => send_input(model, Some(SendMode::Queue), &mut effects),
        ComposerMsg::Abort => {
            if model.connected()
                && model.busy()
                && let Some(id) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::Abort(id)));
            }
        }
        ComposerMsg::AbortAt(clock) => {
            if model
                .composer
                .stop_guard_until
                .is_some_and(|until| clock < until)
            {
                return effects;
            }
            if model.connected()
                && model.busy()
                && let Some(id) = model.selection.selected
            {
                model.composer.stop_guard_until = Some(clock + STOP_GUARD_SECS);
                effects.push(Effect::Server(ServerEffect::Abort(id)));
            }
        }
        ComposerMsg::StartAttachments(source) => {
            if model.selection.selected.is_none()
                && model.selection.new_thread.is_none()
                && model.connected()
            {
                let draft = NewThread::new(model.selection.next_draft, None);
                model.selection.next_draft += 1;
                model.selection.new_thread = Some(draft);
            }
            model.composer.drag_over = false;
            if let Some(target) = model
                .attachment_target()
                .filter(|target| !model.composer.attachment_reads.contains(target))
            {
                model.composer.attachment_reads.insert(target);
                effects.push(Effect::Attach {
                    scope: model.connection.epoch,
                    target,
                    source,
                });
            }
        }
        ComposerMsg::RemoveAttachment(index) => {
            if index < model.composer.draft.attachments.len() {
                model.composer.draft.attachments.remove(index);
                if index < model.composer.draft.attachment_statuses.len() {
                    model.composer.draft.attachment_statuses.remove(index);
                }
                if model.composer.lightbox == Some(index) {
                    model.composer.lightbox = None;
                } else if let Some(open) = &mut model.composer.lightbox
                    && *open > index
                {
                    *open -= 1;
                }
            }
        }
        ComposerMsg::SetDragOver(over) => model.composer.drag_over = over,
        ComposerMsg::OpenLightbox(index) => model.composer.lightbox = index,
        ComposerMsg::ToggleCompactBeforeSend => {
            model.composer.compact_before_send = !model.composer.compact_before_send;
        }
        ComposerMsg::HistoryStep { up } => step_history(model, up, &mut effects),
        ComposerMsg::SteerQueued(run) => {
            if model.connected()
                && let Some(thread) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::SteerQueued { thread, run }));
            }
        }
        ComposerMsg::EditQueued(run) => edit_queued(model, run, &mut effects),
        ComposerMsg::CancelEditQueued => cancel_edit_queued(model, &mut effects),
        ComposerMsg::RemoveQueued(run) => {
            if model.composer.editing_queued == Some(run) {
                cancel_edit_queued(model, &mut effects);
            }
            if model.connected()
                && let Some(thread) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::RemoveQueued { thread, run }));
            }
        }
        ComposerMsg::ToggleQueuedExpanded => {
            model.composer.queued_expanded = !model.composer.queued_expanded;
        }
        ComposerMsg::PollCompletion { clock, cursor } => {
            poll_completion(model, clock, &cursor, &mut effects);
        }
        ComposerMsg::CompletionHover(index) => model.composer.completion.highlighted = index,
        ComposerMsg::CompletionKey { key, cursor } => {
            handle_completion_key(model, key, &cursor, &mut effects);
        }
        ComposerMsg::ChooseCompletion { index, cursor } => {
            choose_completion(model, index, &cursor, &mut effects);
        }
        ComposerMsg::ToggleHomeProjectPicker => toggle_home_project_picker(model, &mut effects),
        ComposerMsg::CloseHomeProjectPicker => close_home_project_picker(model, &mut effects),
        ComposerMsg::HomeProjectQueryChanged(query) => {
            model.composer.home_project.query = query;
            model.composer.home_project.highlighted = 0;
        }
        ComposerMsg::HomeProjectKey(key) => handle_home_project_key(model, key, &mut effects),
        ComposerMsg::ToggleProjectPin(path) => toggle_project_pin(model, path, &mut effects),
        ComposerMsg::ToggleDraftBranchPicker(project) => {
            toggle_draft_branch_picker(model, project, &mut effects);
        }
        ComposerMsg::DraftBranchQueryChanged(query) => {
            model.composer.draft_branch.query = query;
            model.composer.draft_branch.highlighted = 0;
        }
        ComposerMsg::DraftBranchKey(key) => handle_draft_branch_key(model, key, &mut effects),
        ComposerMsg::TogglePicker(key) => {
            let blocked_draft = key == HOME_PROJECT_PICKER
                && model
                    .selection
                    .new_thread
                    .as_ref()
                    .is_some_and(|thread| thread.sending);
            if model.composer.open_picker.as_deref() == Some(key.as_str()) || blocked_draft {
                model.composer.open_picker = None;
            } else {
                model.composer.open_picker = Some(key);
            }
        }
        ComposerMsg::ToggleCheckoutPicker { key, project } => {
            let blocked_draft = key == "draft-checkout"
                && model
                    .selection
                    .new_thread
                    .as_ref()
                    .is_none_or(|draft| draft.sending);
            if model.composer.open_picker.as_deref() == Some(key.as_str()) || blocked_draft {
                model.composer.open_picker = None;
            } else {
                model.composer.open_picker = Some(key);
                effects.push(Effect::Server(ServerEffect::LoadCheckouts(project)));
            }
        }
        ComposerMsg::ClosePicker => {
            model.composer.open_picker = None;
        }
    }
    effects
}
