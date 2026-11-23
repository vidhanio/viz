//! Pure transitions for the keyboard command palette.

use std::time::Duration;

use viz_client::protocol::{Position, TriageAction};

use super::{
    NavKey,
    selection::{apply_thread_change, prepare_draft, select_thread},
    sidebar::open_edit,
};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect, ThreadChange},
    model::{
        Model, ModelMenuState, NewThread, PaletteChoice, PaletteCommand, PaletteSearch, SidebarEdit,
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
                close(model, &mut effects);
            } else {
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
        }
        PaletteMsg::Close => close(model, &mut effects),
        PaletteMsg::QueryChanged { text, clock } => {
            if !model.palette.open {
                return effects;
            }
            model.palette.query = text;
            model.palette.filter = model.palette.query.trim().to_lowercase();
            model.palette.active = None;
            model.palette.invalidate();
            if model.connected() && !model.palette.filter.is_empty() {
                schedule_debounce(model, clock, &mut effects);
            }
            effects.push(Effect::ScrollToInstant {
                key: LIST,
                x: 0.0,
                y: 0.0,
            });
        }
        PaletteMsg::Poll(clock) => poll(model, clock, &mut effects),
        PaletteMsg::Key { key, choices } => handle_key(model, key, &choices, &mut effects),
        PaletteMsg::Choose(choice) => choose(model, choice, &mut effects),
    }
    effects
}

fn close(model: &mut Model, effects: &mut Vec<Effect>) {
    model.palette.open = false;
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
    if !model.palette.open || model.palette.filter.is_empty() {
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
        PaletteChoice::Thread(_) | PaletteChoice::Message { .. } => model.connected(),
    };
    if !available {
        return;
    }
    close(model, effects);
    match choice {
        PaletteChoice::Command(command) => run_command(model, command, effects),
        PaletteChoice::Thread(id) | PaletteChoice::Message { thread: id, .. } => {
            select_thread(model, id, effects);
        }
    }
}

fn run_command(model: &mut Model, command: PaletteCommand, effects: &mut Vec<Effect>) {
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
        PaletteCommand::NewThread => {
            let draft = NewThread::new(model.selection.next_draft, model.project());
            prepare_draft(model, draft, effects);
        }
        PaletteCommand::AddProject => {
            if model.connected() {
                model.sidebar.menu = None;
                effects.push(Effect::PickProjectFolder);
            }
        }
        PaletteCommand::Sidebar => {
            model.panels.sidebar.open = !model.panels.sidebar.open;
            effects.push(Effect::SyncSidebarPane {
                open: Some(model.panels.sidebar.open),
                width: None,
            });
            effects.push(Effect::SavePrefs(model.to_prefs()));
        }
        PaletteCommand::Pin => {
            if let Some((id, pinned, _)) = thread {
                let action = if pinned {
                    TriageAction::Unpin
                } else {
                    TriageAction::Pin {
                        position: Position::Top,
                    }
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
        PaletteCommand::Archive => {
            if let Some((id, _, archived)) = thread {
                let action = if archived {
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
