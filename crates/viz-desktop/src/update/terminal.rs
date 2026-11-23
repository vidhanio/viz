//! Terminal dock transitions and typed connection commands.

use viz_client::protocol::{
    CellSize, CreateTerminal, InteractiveScope, Size, TerminalId, TerminalScope, TerminalSummary,
};

use crate::{
    effects::{Effect, ServerEffect},
    model::{
        Model,
        terminal::{Accessory, Dock, Place, scope},
    },
};

#[derive(Debug)]
pub enum Command {
    List,
    Create(CreateTerminal, Place),
    Attach(TerminalId),
    Detach(TerminalId),
    Input(TerminalId, Vec<u8>),
    Resize(TerminalId, Size, CellSize),
    Close(TerminalId),
}

#[derive(Debug)]
pub enum TerminalMsg {
    Create(InteractiveScope, Place),
    Created(TerminalSummary, Place),
    CreateFailed,
    Listed(Vec<TerminalSummary>),
    Select(TerminalId),
    Close(TerminalId),
    Find(bool),
    Commands,
    Height(f32, bool),
}

pub const fn rpc(command: Command) -> Effect {
    Effect::Server(ServerEffect::Terminal(command))
}

fn create(model: &mut Model, scope: InteractiveScope, place: Place) -> Vec<Effect> {
    if model.terminal.creating.is_some() || !model.connected() {
        return Vec::new();
    }
    model.terminal.creating = Some(place);
    vec![rpc(Command::Create(
        CreateTerminal {
            scope,
            size: Size::default(),
            cell: crate::terminal::CELL,
        },
        place,
    ))]
}

pub fn update(model: &mut Model, msg: TerminalMsg) -> Vec<Effect> {
    match msg {
        TerminalMsg::Create(scope, place) => return create(model, scope, place),
        TerminalMsg::Created(summary, place) => {
            model.terminal.creating = None;
            if model.terminal.summary(summary.id).is_none() {
                let id = summary.id;
                model.terminal.sessions.push(summary);
                model.terminal.place(id, place);
            }
        }
        TerminalMsg::CreateFailed => model.terminal.creating = None,
        TerminalMsg::Listed(sessions) => {
            model.terminal.listed = true;
            model.terminal.sessions = sessions;
            let dock = &mut model.terminal;
            for (_, panes) in &mut dock.groups {
                panes.retain(|id| dock.sessions.iter().any(|summary| summary.id == *id));
            }
            dock.groups.retain(|(_, panes)| panes.len() > 1);
        }
        TerminalMsg::Select(id) => {
            model.terminal.active = Some(id);
            model.terminal.accessory = Accessory::None;
        }
        TerminalMsg::Close(id) => return vec![rpc(Command::Close(id))],
        TerminalMsg::Find(open) => {
            model.terminal.accessory = if open {
                Accessory::Find
            } else {
                Accessory::None
            }
        }
        TerminalMsg::Commands => {
            model.terminal.accessory = if model.terminal.accessory == Accessory::Commands {
                Accessory::None
            } else {
                Accessory::Commands
            }
        }
        TerminalMsg::Height(height, save) => {
            model.panels.terminal_height = height
                .round()
                .clamp(120.0, crate::model::terminal::max_height(model));
            if save {
                return vec![Effect::SavePrefs(model.to_prefs())];
            }
        }
    }
    Vec::new()
}

pub fn reconcile(model: &mut Model, effects: &mut Vec<Effect>) {
    let opening = model.panels.terminal_open && !model.terminal.panel_was_open;
    let closing = !model.panels.terminal_open && model.terminal.panel_was_open;
    let environment = model.active_env_key();
    if model.terminal.environment != environment {
        model.terminal = Dock {
            environment: environment.to_owned(),
            ..Dock::default()
        };
    }
    let connected = model.connected();
    let dock = &mut model.terminal;
    dock.panel_was_open = model.panels.terminal_open;
    if opening {
        dock.auto_create = true;
        if connected {
            dock.listed = false;
            effects.push(rpc(Command::List));
        }
    }
    if closing {
        dock.accessory = Accessory::None;
        dock.auto_create = false;
        effects.push(Effect::Focus {
            key: "draft",
            select_all: false,
            when_shown: false,
        });
    }
    if !connected {
        dock.attached.clear();
        dock.listed = false;
        dock.creating = None;
        return;
    }
    if !model.panels.terminal_open {
        effects.extend(
            std::mem::take(&mut dock.attached)
                .into_iter()
                .map(|id| rpc(Command::Detach(id))),
        );
        return;
    }
    let dock = &model.terminal;
    let visible = |id| {
        dock.summary(id)
            .is_some_and(|summary| Dock::visible(model, summary))
    };
    let active = dock.active.filter(|id| visible(*id)).or_else(|| {
        dock.sessions
            .iter()
            .find(|summary| {
                !matches!(summary.scope, TerminalScope::Agent { .. })
                    && Dock::visible(model, summary)
            })
            .map(|summary| summary.id)
    });
    let desired = active
        .into_iter()
        .flat_map(|id| {
            let (_, panes) = dock.panes(id);
            if panes.is_empty() {
                vec![id]
            } else {
                panes.to_vec()
            }
        })
        .filter(|id| visible(*id))
        .collect::<std::collections::BTreeSet<_>>();
    let dock = &mut model.terminal;
    effects.extend(
        dock.attached
            .difference(&desired)
            .map(|id| rpc(Command::Detach(*id))),
    );
    effects.extend(
        desired
            .difference(&dock.attached)
            .map(|id| rpc(Command::Attach(*id))),
    );
    if active != dock.active || (opening && active.is_some()) {
        dock.accessory = Accessory::None;
    }
    dock.active = active;
    dock.attached = desired;
    if dock.auto_create && dock.listed {
        dock.auto_create = false;
        if active.is_none()
            && let Some(scope) = scope(model, false)
        {
            effects.extend(create(model, scope, Place::Tab));
        }
    }
}
