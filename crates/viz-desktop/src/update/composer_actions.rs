//! Pure composer helper transitions: sending/queueing prompts with
//! `PromptContext`, `@`/`/`/`$`/`#` completion acceptance, prompt history
//! recall, and queued message editing.

use std::{mem, time::Duration};

use viz_client::protocol::{LaunchThread, RunId, SendInput, SendMode};

use super::{
    NavKey,
    selection::{focus_draft, prepare_draft},
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect},
    model::{
        BuiltinCommand, CompletionChoice, CompletionKind, DraftCursor, Item, Model, NewThread,
        completion_choice_at, completion_count, completion_token_at,
    },
};

/// Debounce delay before firing `@` file search RPCs.
const FILE_DELAY: Duration = Duration::from_millis(110);
/// Debounce delay before firing `#` GitHub search RPCs.
const GITHUB_DELAY: Duration = Duration::from_millis(140);

pub(super) fn send_input(
    model: &mut Model,
    explicit_mode: Option<SendMode>,
    effects: &mut Vec<Effect>,
) {
    if !model.can_send() {
        return;
    }
    if let Some(run) = model.composer.editing_queued.take() {
        if let Some(thread) = model.selection.selected {
            let text = model.composer.draft.text.clone();
            let restored = model.composer.editing_stash.take().unwrap_or_default();
            let restore_text = restored.text.clone();
            let restore_pills = restored.pills.clone();
            model.composer.draft = restored;
            model.composer.completion.reset();
            effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
                text: restore_text,
                pills: restore_pills,
            }));
            effects.push(Effect::Server(ServerEffect::UpdateQueued {
                thread,
                run,
                text,
            }));
            effects.push(focus_draft());
        }
        return;
    }
    if model.selection.selected.is_none() && model.selection.new_thread.is_none() {
        let draft = NewThread::new(model.selection.next_draft, None);
        model.selection.next_draft += 1;
        model.selection.new_thread = Some(draft);
    }
    let submitted = mem::take(&mut model.composer.draft);
    let context = submitted.prompt_context();
    let text = submitted.text.clone();
    let attachments = submitted.attachments.clone();
    let command = if let Some(thread) = &mut model.selection.new_thread {
        thread.sending = true;
        ServerEffect::Create {
            draft: thread.id,
            recovery: Box::new(submitted),
            params: Box::new(LaunchThread {
                project: thread.project.clone(),
                worktree: thread.worktree.clone(),
                harness: thread.harness.clone(),
                options: thread.options.clone(),
                access: thread.access.clone(),
                text: Some(text),
                attachments,
                context,
                ..LaunchThread::default()
            }),
        }
    } else if let Some(thread) = model.selection.selected {
        record_prompt_history(model, thread, &text);
        let mode = if model.busy() {
            explicit_mode.or(Some(SendMode::Steer))
        } else {
            None
        };
        ServerEffect::SendInput {
            recovery: Some(Box::new(crate::effects::DraftRecovery {
                environment: model.active_env_key().to_owned(),
                draft: submitted,
            })),
            input: SendInput {
                thread,
                text,
                attachments,
                stored: Vec::new(),
                context,
                request_id: None,
                origin: None,
                mode,
            },
        }
    } else {
        return;
    };
    model.composer.completion.reset();
    model.composer.open_picker = None;
    model.composer.history_cursor = None;
    model.composer.history_stash = None;
    model.composer.compact_before_send = false;
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::Draft)));
    effects.push(Effect::ScrollToBottom("timeline"));
    effects.push(Effect::Server(command));
    effects.push(Effect::SavePrefs(model.to_prefs()));
}

fn record_prompt_history(model: &mut Model, thread: viz_client::protocol::ThreadId, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let list = model.composer.prompt_history.entry(thread).or_default();
    if list.last().map(String::as_str) != Some(trimmed) {
        list.push(trimmed.to_owned());
        if list.len() > 50 {
            list.remove(0);
        }
    }
}

pub(super) fn poll_completion(
    model: &mut Model,
    clock: f64,
    cursor: &DraftCursor,
    effects: &mut Vec<Effect>,
) {
    model.composer.draft_focused = cursor.focused;
    if !model.connected() || model.composer.open_picker.is_some() {
        if model.composer.completion.token.is_some() {
            model.composer.completion.reset();
        }
        return;
    }
    let Some(token) = completion_token_at(
        &model.composer.draft.text,
        cursor,
        &model.composer.draft.pills,
    ) else {
        if model.composer.completion.token.is_some() {
            model.composer.completion.reset();
        }
        return;
    };
    if !model.composer.completion.matches(&token) {
        let kind = token.kind;
        model.composer.completion.reset();
        model.composer.completion.token = Some(token);
        match kind {
            CompletionKind::Mention
                if model.selection.selected.is_some() || model.project_path().is_some() =>
            {
                model.composer.completion.due = Some(clock + FILE_DELAY.as_secs_f64());
                effects.push(Effect::WakeAfter(FILE_DELAY));
            }
            CompletionKind::PullRequest if model.project().is_some_and(|p| p.git) => {
                model.composer.completion.due = Some(clock + GITHUB_DELAY.as_secs_f64());
                effects.push(Effect::WakeAfter(GITHUB_DELAY));
            }
            _ => {}
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
        match token.kind {
            CompletionKind::Mention => {
                let thread = model.selection.selected;
                let project = thread.is_none().then(|| model.project_path()).flatten();
                if thread.is_some() || project.is_some() {
                    effects.push(Effect::Server(ServerEffect::SearchFiles {
                        thread,
                        project: project.map(str::to_owned),
                        request,
                        query: token.query,
                    }));
                }
            }
            CompletionKind::PullRequest => {
                if let Some(project) = model.project().filter(|p| p.git) {
                    effects.push(Effect::Server(ServerEffect::SearchGitHub {
                        project: project.path.clone(),
                        request,
                        query: token.query,
                    }));
                }
            }
            CompletionKind::Slash | CompletionKind::Skill => {}
        }
    }
}

pub(super) fn handle_completion_key(
    model: &mut Model,
    key: NavKey,
    cursor: &DraftCursor,
    effects: &mut Vec<Effect>,
) {
    if model.composer.completion.dismissed {
        return;
    }
    let Some(token) = completion_token_at(
        &model.composer.draft.text,
        cursor,
        &model.composer.draft.pills,
    )
    .filter(|token| model.composer.completion.matches(token)) else {
        return;
    };
    let count = completion_count(model, &token);
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
            choose_completion(model, index, cursor, effects);
        }
        _ => {}
    }
}

pub(super) fn choose_completion(
    model: &mut Model,
    index: usize,
    cursor: &DraftCursor,
    effects: &mut Vec<Effect>,
) {
    let Some(token) = completion_token_at(
        &model.composer.draft.text,
        cursor,
        &model.composer.draft.pills,
    )
    .filter(|token| model.composer.completion.matches(token)) else {
        return;
    };
    let Some(choice) = completion_choice_at(model, &token, index) else {
        return;
    };
    let start = token.range.start;
    let end = token.range.end;
    let trailing = model.composer.draft.text.get(end..).unwrap_or_default();
    let needs_space = !trailing.starts_with(char::is_whitespace);
    model.composer.completion.reset();
    match choice {
        CompletionChoice::Builtin(BuiltinCommand::Model) => {
            clear_token_range(model, start, end, effects);
            effects.extend(super::model_picker::update(
                model,
                super::ModelPickerMsg::Toggle,
            ));
        }
        CompletionChoice::Builtin(BuiltinCommand::Clear) => {
            let project = model.project().cloned();
            let draft = NewThread::new(model.selection.next_draft, project.as_ref());
            prepare_draft(model, draft, effects);
        }
        CompletionChoice::Builtin(BuiltinCommand::Compact) => {
            if let Some(thread) = model.selection.selected.filter(|_| !model.busy()) {
                clear_token_range(model, start, end, effects);
                effects.push(Effect::Server(ServerEffect::SendInput {
                    recovery: None,
                    input: SendInput {
                        thread,
                        text: "/compact".to_owned(),
                        attachments: Vec::new(),
                        stored: Vec::new(),
                        context: Vec::new(),
                        request_id: None,
                        origin: None,
                        mode: None,
                    },
                }));
                effects.push(focus_draft());
            } else {
                replace_plain(model, start, end, "/compact", needs_space, effects);
            }
        }
        CompletionChoice::SlashText(cmd) => {
            replace_plain(model, start, end, &cmd, needs_space, effects);
        }
        CompletionChoice::Pill { id, label } => {
            effects.push(Effect::SyncEdit(EditSync::InsertDraftPill {
                start,
                end,
                id,
                label,
                trailing_space: needs_space,
            }));
            effects.push(focus_draft());
        }
    }
}

fn clear_token_range(model: &mut Model, start: usize, end: usize, effects: &mut Vec<Effect>) {
    if start <= end && end <= model.composer.draft.text.len() {
        model.composer.draft.text.replace_range(start..end, "");
    }
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftRange {
        start,
        end,
        insertion: String::new(),
    }));
}

fn replace_plain(
    model: &mut Model,
    start: usize,
    end: usize,
    text: &str,
    needs_space: bool,
    effects: &mut Vec<Effect>,
) {
    let mut insertion = text.to_owned();
    if needs_space {
        insertion.push(' ');
    }
    if start <= end && end <= model.composer.draft.text.len() {
        model
            .composer
            .draft
            .text
            .replace_range(start..end, &insertion);
    }
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftRange {
        start,
        end,
        insertion,
    }));
    effects.push(focus_draft());
}

pub(super) fn step_history(model: &mut Model, up: bool, effects: &mut Vec<Effect>) {
    let Some(thread) = model.selection.selected else {
        return;
    };
    let entries = thread_prompt_history(model, thread);
    if entries.is_empty() {
        return;
    }
    let next_idx = match (model.composer.history_cursor, up) {
        (None, true) => {
            model.composer.history_stash = Some(model.composer.draft.clone());
            Some(entries.len() - 1)
        }
        (None, false) => return,
        (Some(0), true) => Some(0),
        (Some(idx), true) => Some(idx - 1),
        (Some(idx), false) if idx + 1 < entries.len() => Some(idx + 1),
        (Some(_), false) => None,
    };
    model.composer.history_cursor = next_idx;
    if let Some(idx) = next_idx {
        let text = entries[idx].clone();
        model.composer.draft.text.clone_from(&text);
        model.composer.draft.pills.clear();
        effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
            text,
            pills: Vec::new(),
        }));
    } else if let Some(stashed) = model.composer.history_stash.take() {
        let text = stashed.text.clone();
        let pills = stashed.pills.clone();
        model.composer.draft = stashed;
        effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
            text,
            pills,
        }));
    }
}

fn thread_prompt_history(model: &Model, thread: viz_client::protocol::ThreadId) -> Vec<String> {
    let mut history: Vec<String> = model
        .transcript()
        .items
        .iter()
        .filter_map(|item| match item {
            Item::User(user) if !user.text.trim().is_empty() => Some(user.text.trim().to_owned()),
            _ => None,
        })
        .collect();
    if let Some(local) = model.composer.prompt_history.get(&thread) {
        for prompt in local {
            if history.last() != Some(prompt) {
                history.push(prompt.clone());
            }
        }
    }
    history
}

pub(super) fn edit_queued(model: &mut Model, run: RunId, effects: &mut Vec<Effect>) {
    let Some(text) = model
        .thread_state()
        .and_then(|state| state.queue.iter().find(|item| item.run == run))
        .map(|queued| queued.text.clone())
    else {
        return;
    };
    if model.composer.editing_queued.is_none() {
        model.composer.editing_stash = Some(model.composer.draft.clone());
    }
    model.composer.editing_queued = Some(run);
    model.composer.draft.text.clone_from(&text);
    model.composer.draft.pills.clear();
    model.composer.completion.reset();
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
        text,
        pills: Vec::new(),
    }));
    effects.push(focus_draft());
}

pub(super) fn cancel_edit_queued(model: &mut Model, effects: &mut Vec<Effect>) {
    if model.composer.editing_queued.take().is_none() {
        return;
    }
    let restored = model.composer.editing_stash.take().unwrap_or_default();
    let text = restored.text.clone();
    let pills = restored.pills.clone();
    model.composer.draft = restored;
    effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
        text,
        pills,
    }));
    effects.push(focus_draft());
}
