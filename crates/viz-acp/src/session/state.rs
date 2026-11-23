//! Collects session reports and routes ordered updates to the active or idle
//! fold.

use std::{
    collections::{BTreeMap, HashMap},
    mem,
    sync::{Arc, Mutex, MutexGuard},
};

use agent_client_protocol::schema::MaybeUndefined;
use tokio::sync::{mpsc, watch};
use viz_protocol::ConfigOption;

use super::Session;
use crate::{
    ObservedUpdate, UpdateSink,
    options::{Desired, normalize, with_legacy_modes},
    schema::{
        AvailableCommand, AvailableCommandInput, PlanId, PlanUpdateContent, SessionConfigOption,
        SessionModeState, SessionNotification, SessionUpdate, SubagentUpdate, UsageUpdate,
    },
    settings::Command,
};

/// The session state viz persists and shows, as last reported by the runner.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub options: Vec<ConfigOption>,
    /// Legacy modes reported separately from config options.
    pub modes: Option<SessionModeState>,
    /// Current version of each ID-addressed plan.
    pub plans: HashMap<PlanId, PlanUpdateContent>,
    /// Last reported state of each child session.
    pub subagents: BTreeMap<String, SubagentUpdate>,
    /// The agent's last reported activity timestamp.
    pub updated_at: Option<String>,
    /// Current prompt suggestions, when the harness has supplied them.
    pub suggestions: Option<crate::schema::PromptSuggestionsUpdate>,
    pub commands: Vec<Command>,
    /// Whether the runner takes image prompts.
    pub images: bool,
    pub usage: Option<UsageUpdate>,
    /// The runner's own title for the session, unnormalized.
    pub title: Option<String>,
    /// The values the runner first reported in this process, the catalog's
    /// baseline for unseen options.
    pub opened: Desired,
}

/// Session state the reader task updates as notifications arrive.
#[derive(Default)]
pub(super) struct State {
    /// Updates for other sessions of the process are ignored once the session
    /// is known.
    session_id: Option<String>,
    opening: HashMap<String, Vec<SessionUpdate>>,
    pub(super) options: Vec<ConfigOption>,
    plans: HashMap<PlanId, PlanUpdateContent>,
    subagents: BTreeMap<String, SubagentUpdate>,
    updated_at: Option<String>,
    opened: Option<Desired>,
    pub(super) modes: Option<SessionModeState>,
    mode_update: Option<crate::schema::SessionModeId>,
    suggestions: Option<crate::schema::PromptSuggestionsUpdate>,
    commands: Vec<Command>,
    usage: Option<UsageUpdate>,
    pub(super) turn_usage: Option<UsageUpdate>,
    pub(super) title: Option<String>,
    /// Session-level state changed since the running turn last published.
    pub(super) dirty: bool,
    pub(super) turn: Option<mpsc::UnboundedSender<ObservedUpdate>>,
    /// Installed after opening so session/load replay is discarded.
    idle: Option<mpsc::UnboundedSender<ObservedUpdate>>,
}

impl State {
    pub(super) fn bind(
        &mut self,
        session_id: &str,
        modes: Option<SessionModeState>,
        options: Option<Vec<SessionConfigOption>>,
    ) -> mpsc::UnboundedReceiver<ObservedUpdate> {
        self.session_id = Some(session_id.to_owned());
        let mut opening = mem::take(&mut self.opening);
        if let Some(updates) = opening.remove(session_id) {
            for update in updates {
                let _ = self.apply_report(update);
            }
        }
        let (sender, idle) = mpsc::unbounded_channel();
        self.idle = Some(sender);
        if let Some(mut modes) = modes {
            if let Some(current_id) = self.mode_update.take() {
                modes.current_mode_id = current_id;
            }
            self.modes = Some(modes);
        }
        if let Some(options) = options {
            set_options(self, options);
        }
        idle
    }
}

impl Session {
    #[must_use]
    pub fn snapshot(&self) -> SessionSnapshot {
        let state = lock(&self.state);
        SessionSnapshot {
            session_id: self.id.clone(),
            options: options_with_mode(&state),
            commands: state.commands.clone(),
            modes: state.modes.clone(),
            plans: state.plans.clone(),
            subagents: state.subagents.clone(),
            updated_at: state.updated_at.clone(),
            suggestions: state.suggestions.clone(),
            images: self.agent.capabilities().image,
            usage: state.usage.clone(),
            title: state.title.clone(),
            opened: state.opened.clone().unwrap_or_default(),
        }
    }

    /// Signals session-level changes and queued idle transcript content.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }

    pub fn idle_updates(&mut self) -> Vec<ObservedUpdate> {
        std::iter::from_fn(|| self.idle.try_recv().ok()).collect()
    }

    pub async fn next_idle_update(&mut self) -> Option<ObservedUpdate> {
        tokio::select! {
            update = self.idle.recv() => update,
            _ = self.agent.closed() => None,
        }
    }

    /// The snapshot for a publish: `Some` when session state changed since the
    /// last one.
    #[must_use]
    pub fn take_snapshot(&self) -> Option<SessionSnapshot> {
        let dirty = mem::take(&mut lock(&self.state).dirty);
        dirty.then(|| self.snapshot())
    }
}

pub(super) fn update_sink(
    state: &Arc<Mutex<State>>,
    changes: &Arc<watch::Sender<()>>,
) -> UpdateSink {
    let state = Arc::clone(state);
    let changes = Arc::clone(changes);
    Arc::new(move |notification: SessionNotification, observed_at: i64| {
        let mut state = lock(&state);
        match &state.session_id {
            Some(id) if id.as_str() != notification.session_id.0.as_ref() => return,
            None => {
                if is_report(&notification.update) {
                    state
                        .opening
                        .entry(notification.session_id.0.to_string())
                        .or_default()
                        .push(notification.update);
                }
                return;
            }
            Some(_) => {}
        }
        let report = is_report(&notification.update);
        let content = matches!(
            notification.update,
            SessionUpdate::AgentMessageChunk(_)
                | SessionUpdate::AgentThoughtChunk(_)
                | SessionUpdate::ToolCall(_)
                | SessionUpdate::ToolCallUpdate(_)
                | SessionUpdate::Plan(_)
                | SessionUpdate::PlanUpdate(_)
        );
        if report {
            state.dirty |= state.apply_report(notification.update.clone());
        }
        let mut signal = state.dirty && report;
        if content {
            let update = ObservedUpdate {
                update: notification.update,
                observed_at,
            };
            if let Some(turn) = &state.turn {
                let _ = turn.send(update);
            } else if let Some(idle) = &state.idle {
                let _ = idle.send(update);
                signal = true;
            }
        }
        drop(state);
        if signal {
            changes.send_replace(());
        }
    })
}

const fn is_report(update: &SessionUpdate) -> bool {
    matches!(
        update,
        SessionUpdate::ConfigOptionUpdate(_)
            | SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::SessionInfoUpdate(_)
            | SessionUpdate::CurrentModeUpdate(_)
            | SessionUpdate::PromptSuggestionsUpdate(_)
            | SessionUpdate::UsageUpdate(_)
            | SessionUpdate::PlanUpdate(_)
            | SessionUpdate::PlanRemoved(_)
            | SessionUpdate::SubagentUpdate(_)
    )
}

impl State {
    fn apply_report(&mut self, update: SessionUpdate) -> bool {
        match update {
            SessionUpdate::ConfigOptionUpdate(update) => set_options(self, update.config_options),
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.commands = update.available_commands.into_iter().map(command).collect();
            }
            SessionUpdate::SessionInfoUpdate(update) => {
                patch(&mut self.title, update.title);
                patch(&mut self.updated_at, update.updated_at);
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                if let Some(modes) = &mut self.modes {
                    modes.current_mode_id = update.current_mode_id;
                } else {
                    self.mode_update = Some(update.current_mode_id);
                }
            }
            SessionUpdate::PromptSuggestionsUpdate(update) => self.suggestions = Some(update),
            SessionUpdate::UsageUpdate(update) => {
                if self.turn.is_some() {
                    self.turn_usage = Some(update.clone());
                }
                self.usage = Some(update);
            }
            SessionUpdate::PlanUpdate(update) => {
                let id = match &update.plan {
                    PlanUpdateContent::Items(plan) => &plan.plan_id,
                    PlanUpdateContent::Markdown(plan) => &plan.plan_id,
                    _ => return false,
                };
                self.plans.insert(id.clone(), update.plan);
            }
            SessionUpdate::PlanRemoved(update) => {
                return self.plans.remove(&update.plan_id).is_some();
            }
            SessionUpdate::SubagentUpdate(update) => {
                self.subagents
                    .insert(update.session_id.0.to_string(), update);
            }
            _ => return false,
        }
        true
    }
}

fn patch<T>(value: &mut Option<T>, update: MaybeUndefined<T>) {
    match update {
        MaybeUndefined::Value(new) => *value = Some(new),
        MaybeUndefined::Null => *value = None,
        MaybeUndefined::Undefined => {}
    }
}

pub(super) fn options_with_mode(state: &State) -> Vec<ConfigOption> {
    with_legacy_modes(state.options.clone(), state.modes.as_ref())
}

pub(super) fn set_options(state: &mut State, options: Vec<SessionConfigOption>) {
    let options = normalize(options);
    state.opened.get_or_insert_with(|| {
        options
            .iter()
            .map(|option| (option.id.clone(), option.value.clone()))
            .collect()
    });
    state.options = options;
}

fn command(command: AvailableCommand) -> Command {
    let hint = match command.input {
        Some(AvailableCommandInput::Unstructured(input)) => Some(input.hint),
        _ => None,
    };
    let skill = command
        .meta
        .as_ref()
        .and_then(|meta| meta.get("jetbrains"))
        .and_then(|jetbrains| jetbrains.get("air"))
        .filter(|air| air.get("kind").and_then(serde_json::Value::as_str) == Some("skill"))
        .map(|air| crate::settings::Skill {
            path: air
                .get("skillPath")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        });
    Command {
        skill,
        meta: command.meta,
        name: command.name,
        description: command.description,
        hint,
    }
}

pub(super) fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().expect("ACP session state lock poisoned")
}
