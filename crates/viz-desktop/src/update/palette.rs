//! Pure transitions for the keyboard command palette.

use std::time::Duration;

use viz_client::protocol::{EntryAnchor, EntryId, Position, ThreadId, TriageAction};

use super::{
    NavKey, projects,
    selection::{apply_thread_change, prepare_draft, select_thread},
    sidebar::open_edit,
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect, ThreadChange},
    model::{
        Model, ModelMenuState, NewThread, PaletteChoice, PaletteCommand, PaletteSearch,
        SidebarEdit,
        projects::{Destination, ProjectAction, ProjectView},
    },
};

const FIELD: &str = "palette-query";
const LIST: &str = "palette-results";
const SEARCH_DELAY: Duration = Duration::from_millis(180);

/// Messages emitted by the command palette modal and global shortcut.
#[derive(Debug)]
pub enum PaletteMsg {
    /// Open or dismiss the palette (`Cmd/Ctrl+K`).
    Toggle,
    OpenProject(ProjectView),
    Project(ProjectAction),
    Highlight(usize),
    Back,
    /// Dismiss the palette and reset query state.
    Close,
    /// Query text changed; resets highlight and schedules debounced search.
    QueryChanged {
        /// New input text.
        text: String,
        /// Current UI clock in seconds.
        clock: f64,
    },
    /// Poll debounced transcript search timer.
    Poll(f64),
    /// Navigation key pressed inside the palette query field.
    Key {
        /// Navigation key.
        key: NavKey,
        /// Visible choices in display order.
        choices: Vec<PaletteChoice>,
    },
    /// Activate a palette row.
    Choose(PaletteChoice),
}

pub(super) fn update(model: &mut Model, msg: PaletteMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        PaletteMsg::Toggle => {
            if model.palette.open {
                dismiss(model, &mut effects);
            } else {
                open(model, &mut effects);
            }
        }
        PaletteMsg::OpenProject(view) => {
            if model.connected() {
                open(model, &mut effects);
                if !projects::clone_active(model) {
                    projects::push(model, view, String::new(), &mut effects);
                }
            }
        }
        PaletteMsg::Project(action) => projects::action(model, action, &mut effects),
        PaletteMsg::Highlight(index) => projects::highlight(model, index, &mut effects),
        PaletteMsg::Back => projects::back(model, &mut effects),
        PaletteMsg::Close => dismiss(model, &mut effects),
        PaletteMsg::QueryChanged { text, clock } => {
            if !model.palette.open {
                return effects;
            }
            if projects::page(model).is_some() {
                projects::query(model, text, clock, &mut effects);
                return effects;
            }
            if model.connected()
                && ["/", "~/", "./", "../"]
                    .iter()
                    .any(|prefix| text.starts_with(prefix))
            {
                projects::push(
                    model,
                    ProjectView::Folders(Destination::Add),
                    text,
                    &mut effects,
                );
                return effects;
            }
            model.palette.query = text;
            model.palette.filter = model.palette.query.trim().to_lowercase();
            model.palette.active = None;
            model.palette.invalidate();
            if model.connected()
                && !model.palette.filter.is_empty()
                && !model.palette.filter.starts_with('>')
            {
                schedule_debounce(model, clock, &mut effects);
            }
            effects.push(Effect::ScrollToInstant {
                key: LIST,
                x: 0.0,
                y: 0.0,
            });
        }
        PaletteMsg::Poll(clock) => {
            if projects::page(model).is_some() {
                projects::poll(model, clock, &mut effects);
            } else {
                poll(model, clock, &mut effects);
            }
        }
        PaletteMsg::Key { key, choices } => handle_key(model, key, &choices, &mut effects),
        PaletteMsg::Choose(choice) => choose(model, choice, &mut effects),
    }
    effects
}

fn open(model: &mut Model, effects: &mut Vec<Effect>) {
    model.sidebar.dismiss_overlays();
    model.composer.open_picker = None;
    model.composer.model_menu = ModelMenuState::default();
    model.composer.completion.reset();
    model.palette.open = true;
    effects.push(Effect::Focus {
        key: FIELD,
        select_all: false,
        when_shown: true,
    });
}

fn dismiss(model: &mut Model, effects: &mut Vec<Effect>) {
    if projects::clone_active(model) {
        model.palette.open = false;
    } else {
        close(model, effects);
    }
}

pub(super) fn close(model: &mut Model, effects: &mut Vec<Effect>) {
    model.palette.open = false;
    model.palette.pages.stack.clear();
    model.palette.pages.due = None;
    model.palette.pages.request = model.palette.pages.request.wrapping_add(1);
    model.palette.query.clear();
    model.palette.filter.clear();
    model.palette.active = None;
    model.palette.invalidate();
    effects.push(Effect::SyncEdit(EditSync::Clear(EditTarget::PaletteQuery)));
}

fn schedule_debounce(model: &mut Model, clock: f64, effects: &mut Vec<Effect>) {
    model.palette.search = PaletteSearch::Debouncing(clock + SEARCH_DELAY.as_secs_f64());
    effects.push(Effect::WakeAfter(SEARCH_DELAY));
}

fn poll(model: &mut Model, clock: f64, effects: &mut Vec<Effect>) {
    if !model.palette.open
        || model.palette.filter.is_empty()
        || model.palette.filter.starts_with('>')
    {
        return;
    }
    if !model.connected() {
        if !matches!(model.palette.search, PaletteSearch::Idle) {
            model.palette.invalidate();
        }
        return;
    }
    match model.palette.search {
        PaletteSearch::Idle => schedule_debounce(model, clock, effects),
        PaletteSearch::Debouncing(due) if clock >= due => {
            let request = model.palette.request;
            let query = model.palette.query.trim().to_owned();
            model.palette.search = PaletteSearch::Pending;
            effects.push(Effect::Server(ServerEffect::SearchThreads {
                request,
                query,
            }));
        }
        _ => {}
    }
}

fn handle_key(
    model: &mut Model,
    key: NavKey,
    choices: &[PaletteChoice],
    effects: &mut Vec<Effect>,
) {
    if choices.is_empty() {
        return;
    }
    let active = model
        .palette
        .active
        .and_then(|choice| choices.iter().position(|&other| other == choice))
        .unwrap_or_default();
    match key {
        NavKey::Enter => choose(model, choices[active], effects),
        NavKey::Down | NavKey::Up => {
            let next = if key == NavKey::Down {
                (active + 1) % choices.len()
            } else {
                active.checked_sub(1).unwrap_or(choices.len() - 1)
            };
            let choice = choices[next];
            model.palette.active = Some(choice);
            effects.push(Effect::ScrollIntoView {
                list: LIST,
                item: choice.key(),
            });
        }
        NavKey::Escape | NavKey::Tab => {}
    }
}

fn choose(model: &mut Model, choice: PaletteChoice, effects: &mut Vec<Effect>) {
    if !model.palette.open {
        return;
    }
    let available = match choice {
        PaletteChoice::Command(command) => command.available(model),
        PaletteChoice::Project(_) | PaletteChoice::Thread(_) | PaletteChoice::Message { .. } => {
            model.connected()
        }
    };
    if !available {
        return;
    }
    match choice {
        PaletteChoice::Project(index) => {
            let path = model
                .cache()
                .projects()
                .nth(index)
                .map(|project| project.path.clone());
            if let Some(project) = path {
                projects::push(
                    model,
                    ProjectView::Settings(project),
                    String::new(),
                    effects,
                );
            }
        }
        PaletteChoice::Command(command) => run_command(model, command, effects),
        PaletteChoice::Thread(id) => {
            close(model, effects);
            select_thread(model, id, effects);
        }
        PaletteChoice::Message { thread, entry } => {
            close(model, effects);
            if model.selection.selected != Some(thread) {
                select_thread(model, thread, effects);
            }
            model.palette.message_jump = Some((thread, EntryId(entry)));
            continue_message_jump(model, effects);
        }
    }
}

fn run_command(model: &mut Model, command: PaletteCommand, effects: &mut Vec<Effect>) {
    let view = command_view(model, command);
    if let Some(view) = view {
        projects::push(model, view, String::new(), effects);
        return;
    }
    if command == PaletteCommand::Promote {
        projects::promote(model, effects);
        return;
    }
    close(model, effects);
    let thread = model
        .selection
        .selected
        .and_then(|id| model.summary(id))
        .map(|thread| {
            (
                thread.id,
                thread.lifecycle.pin.is_some(),
                thread.lifecycle.archived,
            )
        });
    match command {
        PaletteCommand::AddProject
        | PaletteCommand::Projects
        | PaletteCommand::Settings
        | PaletteCommand::Import
        | PaletteCommand::Environments
        | PaletteCommand::Promote => {}
        PaletteCommand::NewThread => {
            let draft = NewThread::new(model.selection.next_draft, model.project());
            prepare_draft(model, draft, effects);
        }
        PaletteCommand::Scratch => {
            prepare_draft(
                model,
                NewThread::new(model.selection.next_draft, None),
                effects,
            );
        }
        PaletteCommand::Diff => {
            effects.extend(super::git::update(model, super::GitMsg::ToggleDiff));
        }
        PaletteCommand::Sidebar => {
            model.panels.sidebar.open = !model.panels.sidebar.open;
            effects.push(Effect::SyncSidebarPane {
                open: Some(model.panels.sidebar.open),
                width: None,
            });
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
        PaletteCommand::Pin | PaletteCommand::Archive => {
            if let Some((id, pinned, archived)) = thread {
                let action = if command == PaletteCommand::Pin {
                    if pinned {
                        TriageAction::Unpin
                    } else {
                        TriageAction::Pin {
                            position: Position::Top,
                        }
                    }
                } else if archived {
                    TriageAction::Unarchive
                } else {
                    TriageAction::Archive
                };
                apply_thread_change(
                    model,
                    id,
                    ThreadChange::Triage(action),
                    model.disclosure.tool_clock,
                    effects,
                );
            }
        }
        PaletteCommand::Delete => {
            if let Some((id, _, _)) = thread {
                open_edit(model, SidebarEdit::DeleteThread(id), String::new(), effects);
            }
        }
        PaletteCommand::Stop => {
            if model.connected()
                && model.busy()
                && let Some(id) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::Abort(id)));
            }
        }
    }
}

fn command_view(model: &Model, command: PaletteCommand) -> Option<ProjectView> {
    match command {
        PaletteCommand::AddProject => Some(ProjectView::Sources),
        PaletteCommand::Projects => Some(ProjectView::Projects),
        PaletteCommand::Settings => model
            .project()
            .map(|project| ProjectView::Settings(project.path.clone())),
        PaletteCommand::Import => Some(ProjectView::Harnesses),
        PaletteCommand::Environments => Some(ProjectView::Environments),
        _ => None,
    }
}

/// Resolves a search hit in the current window or fetches one centered page.
pub(super) fn continue_message_jump(model: &mut Model, effects: &mut Vec<Effect>) {
    let Some((thread, entry)) = model.palette.message_jump else {
        return;
    };
    if model.selection.selected != Some(thread) {
        model.palette.message_jump = None;
        return;
    }
    if model.is_loading() {
        return;
    }
    model.palette.message_jump = None;
    if let Some(target) = crate::timeline::message_row(model.transcript(), entry) {
        if matches!(model.history_request(), Some(EntryAnchor::Around { .. })) {
            model.cache_mut().open_thread_mut(thread).history_request = None;
        }
        reveal_message(model, thread, target, effects);
    } else if model.connected() {
        request_history(model, thread, EntryAnchor::Around { entry }, effects);
    }
}

pub(super) fn request_history(
    model: &mut Model,
    thread: ThreadId,
    anchor: EntryAnchor,
    effects: &mut Vec<Effect>,
) {
    let open = model.cache_mut().open_thread_mut(thread);
    if open.history_request == Some(anchor) {
        return;
    }
    open.history_request = Some(anchor);
    effects.push(Effect::Server(ServerEffect::LoadHistory { thread, anchor }));
}

pub(super) fn reveal_message(
    model: &mut Model,
    thread: ThreadId,
    target: crate::timeline::MessageTarget,
    effects: &mut Vec<Effect>,
) {
    for key in target.folds.into_iter().flatten() {
        model.set_fold(Some(thread), key, true);
    }
    effects.push(Effect::RevealMessage {
        thread,
        item: target.selector,
    });
}
