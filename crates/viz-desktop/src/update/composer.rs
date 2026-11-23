//! Pure transitions for the composer: prompt text, attachments, sending and
//! steering, inline completions, inline pickers, and tasks dock actions.

use std::{mem, time::Duration};

use viz_client::protocol::{LaunchThread, SendInput, SendMode};

use super::{NavKey, selection::focus_draft};
use crate::{
    effects::{AttachmentSource, EditSync, EditTarget, Effect, ServerEffect},
    model::{
        CompletionKind, DraftCursor, Model, completion_token_at, format_completion_insertion,
        matching_commands, matching_files,
    },
};

const FILE_DELAY: Duration = Duration::from_millis(110);

/// Messages emitted by the composer, pickers, completion drawer, and tasks
/// dock.
#[derive(Debug)]
pub enum ComposerMsg {
    /// Prompt text edited by the user.
    DraftChanged(String),
    /// Submit the prompt (launch draft or send/steer thread).
    SendInput,
    /// Abort the active turn on the selected thread.
    Abort,
    /// Start loading attachments from file picker, drag-drop, or clipboard.
    StartAttachments(AttachmentSource),
    /// Remove an attached file by index.
    RemoveAttachment(usize),
    /// Periodic completion poll syncing the caret and firing debounced file
    /// search.
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
    /// User picked completion item `id`.
    ChooseCompletion {
        /// Command name or file path.
        id: String,
        /// Current composer cursor snapshot.
        cursor: DraftCursor,
    },
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
    /// Cancel a running or queued delegated task from the Tasks Dock.
    CancelTask(String),
}

pub(super) fn update(model: &mut Model, msg: ComposerMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        ComposerMsg::DraftChanged(text) => {
            model.composer.draft.text = text;
        }
        ComposerMsg::SendInput => send_input(model, &mut effects),
        ComposerMsg::Abort => {
            if model.connected()
                && model.busy()
                && let Some(id) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::Abort(id)));
            }
        }
        ComposerMsg::StartAttachments(source) => {
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
            }
        }
        ComposerMsg::PollCompletion { clock, cursor } => {
            poll_completion(model, clock, &cursor, &mut effects);
        }
        ComposerMsg::CompletionHover(index) => {
            model.composer.completion.highlighted = index;
        }
        ComposerMsg::CompletionKey { key, cursor } => {
            handle_completion_key(model, key, &cursor, &mut effects);
        }
        ComposerMsg::ChooseCompletion { id, cursor } => {
            choose_completion(model, &id, &cursor, &mut effects);
        }
        ComposerMsg::TogglePicker(key) => {
            let blocked_draft = key == "draft-project"
                && model
                    .selection
                    .new_thread
                    .as_ref()
                    .is_none_or(|thread| thread.sending);
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
        ComposerMsg::CancelTask(id) => {
            effects.push(Effect::Server(ServerEffect::CancelTask(id)));
        }
    }
    effects
}

fn send_input(model: &mut Model, effects: &mut Vec<Effect>) {
    if !model.can_send() {
        return;
    }
    let command = if let Some(thread) = &mut model.selection.new_thread {
        thread.sending = true;
        ServerEffect::Create {
            draft: thread.id,
            params: LaunchThread {
                project: thread.project.clone(),
                worktree: thread.worktree.clone(),
                harness: thread.harness.clone(),
                options: thread.options.clone(),
                text: Some(model.composer.draft.text.clone()),
                attachments: mem::take(&mut model.composer.draft.attachments),
                ..LaunchThread::default()
            },
        }
    } else if let Some(thread) = model.selection.selected {
        let mode = model.busy().then_some(SendMode::Steer);
        ServerEffect::SendInput(SendInput {
            thread,
            text: model.composer.draft.text.clone(),
            attachments: mem::take(&mut model.composer.draft.attachments),
            stored: Vec::new(),
            context: Vec::new(),
            request_id: None,
            origin: None,
            mode,
        })
    } else {
        return;
    };
    model.composer.draft.text.clear();
    model.composer.completion.reset();
    model.composer.open_picker = None;
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::Draft)));
    effects.push(Effect::ScrollToBottom("timeline"));
    effects.push(Effect::Server(command));
    effects.push(Effect::SavePrefs(model.to_prefs()));
}

fn poll_completion(model: &mut Model, clock: f64, cursor: &DraftCursor, effects: &mut Vec<Effect>) {
    model.composer.draft_focused = cursor.focused;
    let Some(thread) = model.selection.selected.filter(|_| {
        model.connected() && model.composer.open_picker.is_none() && model.thread_state().is_some()
    }) else {
        if model.composer.completion.token.is_some() {
            model.composer.completion.reset();
        }
        return;
    };
    let Some(token) = completion_token_at(&model.composer.draft.text, cursor) else {
        if model.composer.completion.token.is_some() {
            model.composer.completion.reset();
        }
        return;
    };
    if !model.composer.completion.matches(&token) {
        let kind = token.kind;
        model.composer.completion.reset();
        model.composer.completion.token = Some(token);
        if kind == CompletionKind::File {
            model.composer.completion.due = Some(clock + FILE_DELAY.as_secs_f64());
            effects.push(Effect::WakeAfter(FILE_DELAY));
        }
        return;
    }
    if model
        .composer
        .completion
        .due
        .is_some_and(|due| clock >= due)
    {
        model.composer.completion.due = None;
        let request = model.composer.completion.request;
        effects.push(Effect::Server(ServerEffect::SearchFiles {
            thread,
            request,
            query: token.query,
        }));
    }
}

fn handle_completion_key(
    model: &mut Model,
    key: NavKey,
    cursor: &DraftCursor,
    effects: &mut Vec<Effect>,
) {
    if model.composer.completion.dismissed {
        return;
    }
    let Some(token) = completion_token_at(&model.composer.draft.text, cursor)
        .filter(|token| model.composer.completion.matches(token))
    else {
        return;
    };
    let count = match token.kind {
        CompletionKind::Slash => matching_commands(model, &token.query).len(),
        CompletionKind::File => matching_files(model).len(),
    };
    match key {
        NavKey::Escape => {
            model.composer.completion.dismissed = true;
            model.composer.completion.due = None;
        }
        NavKey::Down | NavKey::Up if count > 0 => {
            let index = model.composer.completion.highlighted.min(count - 1);
            model.composer.completion.highlighted = if key == NavKey::Down {
                (index + 1) % count
            } else if index == 0 {
                count - 1
            } else {
                index - 1
            };
        }
        NavKey::Enter | NavKey::Tab if count > 0 => {
            let index = model.composer.completion.highlighted.min(count - 1);
            let choice = match token.kind {
                CompletionKind::Slash => matching_commands(model, &token.query)
                    .get(index)
                    .map(|cmd| cmd.name.clone()),
                CompletionKind::File => matching_files(model).get(index).cloned(),
            };
            if let Some(choice) = choice {
                choose_completion(model, &choice, cursor, effects);
            }
        }
        _ => {}
    }
}

fn choose_completion(model: &mut Model, id: &str, cursor: &DraftCursor, effects: &mut Vec<Effect>) {
    let Some(token) = completion_token_at(&model.composer.draft.text, cursor)
        .filter(|token| model.composer.completion.matches(token))
    else {
        return;
    };
    let start = token.range.start;
    let end = token.range.end;
    let trailing = model.composer.draft.text.get(end..).unwrap_or_default();
    let insertion = format_completion_insertion(token.kind, id, trailing);
    if start <= end && end <= model.composer.draft.text.len() {
        model
            .composer
            .draft
            .text
            .replace_range(start..end, &insertion);
    }
    model.composer.completion.reset();
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftRange {
        start,
        end,
        insertion,
    }));
    effects.push(focus_draft());
}
