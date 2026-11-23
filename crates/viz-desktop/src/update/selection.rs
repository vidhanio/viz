//! Pure transitions for selection, thread navigation, local drafts, and
//! environment switching.

use std::mem;

use viz_client::protocol::{ThreadId, WorktreeTarget};

use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect, ThreadChange},
    model::{DraftData, Model, NewThread, OptimisticUndo, PaneSwitch, Status, top_level},
};

/// Messages governing the active thread, draft, checkout target, and
/// environment.
#[derive(Debug)]
pub enum SelectionMsg {
    /// Open `thread`, recording the previous selection in back history.
    Select(ThreadId),
    /// Navigate to the previous thread in history.
    GoBack,
    /// Navigate forward in history.
    GoForward,
    /// Start a fresh draft in the active thread's project, if any.
    CreateHere,
    /// Start a fresh draft in the project identified by `path`.
    CreateInProject(String),
    /// Set or clear the active draft's project path.
    ChooseDraftProject(Option<String>),
    /// Set the active draft's Git worktree target.
    ChooseDraftWorktree(WorktreeTarget),
    /// Set the active draft's harness id.
    ChooseDraftHarness(String),
    /// Move an idle thread to another Git worktree.
    MoveThread {
        /// Thread id.
        thread: ThreadId,
        /// Target checkout.
        worktree: WorktreeTarget,
    },
    /// Switch to another environment and optionally open `thread` there.
    OpenInEnvironment {
        /// Target environment id.
        environment: String,
        /// Optional thread to select after connecting.
        thread: Option<ThreadId>,
    },
    /// Retry connecting to the server immediately.
    Reconnect,
}

pub(super) fn update(model: &mut Model, msg: SelectionMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        SelectionMsg::Select(thread) => select_thread(model, thread, &mut effects),
        SelectionMsg::GoBack => travel(model, true, &mut effects),
        SelectionMsg::GoForward => travel(model, false, &mut effects),
        SelectionMsg::CreateHere => {
            let draft = NewThread::new(model.selection.next_draft, model.project());
            prepare_draft(model, draft, &mut effects);
        }
        SelectionMsg::CreateInProject(path) => {
            if let Some(project) = model.cache().project(&path).cloned() {
                let draft = NewThread::new(model.selection.next_draft, Some(&project));
                prepare_draft(model, draft, &mut effects);
            }
        }
        SelectionMsg::ChooseDraftProject(path) => {
            model.composer.open_picker = None;
            let project = path
                .as_deref()
                .and_then(|path| model.cache().project(path))
                .cloned();
            if let Some(thread) = model
                .selection
                .new_thread
                .as_mut()
                .filter(|thread| !thread.sending)
            {
                thread.set_project(project.as_ref());
            }
            effects.push(focus_draft());
        }
        SelectionMsg::ChooseDraftWorktree(target) => {
            model.composer.open_picker = None;
            if let Some(draft) = model
                .selection
                .new_thread
                .as_mut()
                .filter(|draft| !draft.sending)
            {
                draft.worktree = (target != WorktreeTarget::Local).then_some(target);
            }
            effects.push(focus_draft());
        }
        SelectionMsg::ChooseDraftHarness(id) => {
            model.composer.open_picker = None;
            let is_default = model.harnesses().iter().any(|h| h.id == id && h.default);
            if let Some(thread) = &mut model.selection.new_thread {
                let already_selected = thread
                    .harness
                    .as_deref()
                    .map_or(is_default, |picked| picked == id);
                if !already_selected {
                    thread.harness = Some(id);
                    thread.options.clear();
                }
            }
            effects.push(focus_draft());
        }
        SelectionMsg::MoveThread { thread, worktree } => {
            model.composer.open_picker = None;
            effects.push(Effect::Server(ServerEffect::Move { thread, worktree }));
        }
        SelectionMsg::OpenInEnvironment {
            environment,
            thread,
        } => open_in_environment(model, environment, thread, &mut effects),
        SelectionMsg::Reconnect => {
            model.connection.status = Status::Connecting;
            effects.push(Effect::SetWindowTitle(model.window_title()));
            effects.push(Effect::Server(ServerEffect::Reconnect));
        }
    }
    effects
}

pub(super) const fn focus_draft() -> Effect {
    Effect::Focus {
        key: "draft",
        select_all: false,
        when_shown: false,
    }
}

pub(super) fn select_thread(model: &mut Model, thread: ThreadId, effects: &mut Vec<Effect>) {
    if let Some(current) = model
        .selection
        .selected
        .filter(|current| *current != thread)
    {
        model.selection.back.push(current);
        model.selection.forward.clear();
    }
    open_thread(model, thread, effects);
}

fn travel(model: &mut Model, back: bool, effects: &mut Vec<Effect>) {
    let mut next = None;
    loop {
        let candidate = if back {
            model.selection.back.pop()
        } else {
            model.selection.forward.pop()
        };
        let Some(candidate) = candidate else {
            break;
        };
        if model.summary(candidate).is_some() {
            next = Some(candidate);
            break;
        }
    }
    let Some(next) = next else {
        return;
    };
    if let Some(current) = model.selection.selected {
        if back {
            model.selection.forward.push(current);
        } else {
            model.selection.back.push(current);
        }
    }
    open_thread(model, next, effects);
}

pub(super) fn open_thread(model: &mut Model, thread: ThreadId, effects: &mut Vec<Effect>) {
    show_thread(model, thread, effects);
    effects.push(Effect::SetWindowTitle(model.window_title()));
    effects.push(focus_draft());
    effects.push(Effect::Server(ServerEffect::Select(thread)));
    if model.connected()
        && model
            .summary(thread)
            .is_some_and(|summary| summary.lifecycle.unread)
    {
        apply_thread_change(model, thread, ThreadChange::Unread(false), effects);
    }
}

/// Switches visible selection to `thread`, swapping saved drafts and setting
/// loading state until its transcript view arrives.
pub(super) fn show_thread(model: &mut Model, thread: ThreadId, effects: &mut Vec<Effect>) {
    if model.selection.selected == Some(thread) && model.selection.new_thread.is_none() {
        return;
    }
    model.selection.switch = if model.selection.new_thread.is_some() {
        PaneSwitch::Fade
    } else {
        model
            .selection
            .selected
            .map_or(PaneSwitch::Cut, |from| model.switch_direction(from, thread))
    };
    let draft_saved = save_draft(model);
    model.selection.new_thread = None;
    model.selection.selected = Some(thread);
    let env = model.active_env_key().to_owned();
    let restored = model
        .composer
        .drafts_by_env
        .get_mut(&env)
        .and_then(|drafts| drafts.remove(&thread))
        .unwrap_or_default();
    let text = restored.text.clone();
    model.composer.draft = restored;
    reset_active_view(model);
    let open = model.cache_mut().open_thread_mut(thread);
    open.transcript = crate::model::Transcript::default();
    open.state = None;
    open.loading = true;
    open.history_loading = false;
    let expanded_project = reveal_project(model, thread);
    effects.push(Effect::SyncEdit(EditSync::Replace {
        target: EditTarget::Draft,
        text,
        select_all: false,
    }));
    if expanded_project || draft_saved {
        effects.push(Effect::SavePrefs(model.to_prefs()));
    }
}

pub(super) fn save_draft(model: &mut Model) -> bool {
    let draft = mem::take(&mut model.composer.draft);
    let Some(id) = model.selection.selected.take() else {
        return false;
    };
    let env = model.active_env_key().to_owned();
    if draft.is_empty() {
        model
            .composer
            .drafts_by_env
            .get_mut(&env)
            .and_then(|drafts| drafts.remove(&id))
            .is_some()
    } else {
        let changed = model
            .composer
            .drafts_by_env
            .get(&env)
            .and_then(|drafts| drafts.get(&id))
            .is_none_or(|prev| prev.text != draft.text);
        model
            .composer
            .drafts_by_env
            .entry(env)
            .or_default()
            .insert(id, draft);
        changed
    }
}

pub(super) fn clear_selection(model: &mut Model, effects: &mut Vec<Effect>) {
    let had_text = !model.composer.draft.text.is_empty();
    let saved = save_draft(model);
    reset_active_view(model);
    if had_text {
        effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::Draft)));
    }
    if saved {
        effects.push(Effect::SavePrefs(model.to_prefs()));
    }
}

pub(super) fn reset_active_view(model: &mut Model) {
    model.disclosure.expanded.clear();
    model.composer.completion.reset();
    model.composer.open_picker = None;
}

pub(super) fn reveal_project(model: &mut Model, id: ThreadId) -> bool {
    let top = top_level(model.cache(), id, model.connection.environment.as_deref());
    let project = model.summary(top).and_then(|thread| thread.project.clone());
    if let Some(project) = project {
        return model.sidebar.collapsed_projects.remove(&project);
    }
    false
}

pub(super) fn prepare_draft(model: &mut Model, mut thread: NewThread, effects: &mut Vec<Effect>) {
    if !model.connected() || model.selection.new_thread.is_some() {
        return;
    }
    model.selection.next_draft += 1;
    thread.loading_choices = true;
    effects.push(Effect::Server(ServerEffect::LoadDraft(thread.id)));
    model.selection.switch = PaneSwitch::Fade;
    clear_selection(model, effects);
    effects.push(Effect::Server(ServerEffect::ClearSelection));
    model.selection.new_thread = Some(thread);
    effects.push(Effect::SetWindowTitle(model.window_title()));
    effects.push(focus_draft());
}

pub(super) fn open_in_environment(
    model: &mut Model,
    environment: String,
    thread: Option<ThreadId>,
    effects: &mut Vec<Effect>,
) {
    model.sidebar.env_menu = false;
    if model.connection.environment.as_deref() == Some(environment.as_str()) {
        if let Some(thread) = thread {
            select_thread(model, thread, effects);
        }
        return;
    }
    let saved = save_draft(model);
    model.connection.status = Status::Connecting;
    model.connection.environment = Some(environment.clone());
    model.connection.epoch = model.connection.epoch.wrapping_add(1);
    model.selection.new_thread = None;
    model.selection.switch = PaneSwitch::Cut;
    model.selection.back.clear();
    model.selection.forward.clear();
    model.composer.draft = DraftData::default();
    model.composer.attachment_reads.clear();
    model.optimistic.clear();
    model.cache_mut().clear();
    model.sidebar.dismiss_overlays();
    model.palette = crate::model::PaletteState::default();
    reset_active_view(model);
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::Draft)));
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::PaletteQuery)));
    effects.push(Effect::SetWindowTitle(model.window_title()));
    effects.push(Effect::Server(ServerEffect::Environment {
        environment,
        thread,
    }));
    if saved {
        effects.push(Effect::SavePrefs(model.to_prefs()));
    }
}

/// Applies a thread metadata/triage change optimistically to the cache,
/// recording its undo snapshot and emitting the corresponding server RPC.
pub(super) fn apply_thread_change(
    model: &mut Model,
    thread: ThreadId,
    change: ThreadChange,
    effects: &mut Vec<Effect>,
) {
    let env = model.active_env_key().to_owned();
    let op_id = if let Some(prior) = model.summary(thread).cloned() {
        if let Some(summary) = model.cache_mut().summary_mut(thread) {
            match &change {
                ThreadChange::Title(title) => summary.title.clone_from(title),
                ThreadChange::Archive(archived) => summary.lifecycle.archived = *archived,
                ThreadChange::Pin(pinned) => {
                    summary.lifecycle.pin = pinned.then_some(0);
                }
                ThreadChange::Unread(unread) => summary.lifecycle.unread = *unread,
                ThreadChange::Project(project) => summary.project.clone_from(project),
            }
        }
        model.optimistic.record(OptimisticUndo::ThreadSummary {
            env,
            id: thread,
            prior: Box::new(prior),
        })
    } else {
        0
    };
    effects.push(Effect::Server(ServerEffect::Update {
        op_id,
        thread,
        change,
    }));
}
