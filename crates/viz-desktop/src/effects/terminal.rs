//! Terminal RPCs and ordered delivery to the Ghostty viewport.

use std::sync::Arc;

use viz_client::{
    Error,
    protocol::{
        AttachTerminal, CloseTerminal, DetachTerminal, ListTerminals, ResizeTerminal, Snapshot,
        TerminalFailure, TerminalId, TerminalInput, Update,
    },
};

use super::server::{Session, toast};
use crate::update::{
    Msg,
    terminal::{Command, TerminalMsg},
};

pub(super) enum Event {
    Dock(TerminalMsg),
    Snapshot(TerminalId, Snapshot),
    Update(TerminalId, Update),
    AttachFailed(TerminalId, String),
    ResizeLease(TerminalId),
}

impl Session<'_> {
    pub(super) fn terminal_event(&self, event: Event) {
        let environment = Arc::clone(&self.environment);
        self.ui.update(move |app, tx| {
            if environment.as_ref() != app.model.active_env_key() || !app.model.connected() {
                return;
            }
            match event {
                Event::Dock(msg) => app.dispatch(Msg::Terminal(msg), tx),
                Event::Snapshot(id, snapshot) => {
                    crate::terminal::panel::snapshot(app, id, &snapshot, tx);
                }
                Event::Update(id, update) => crate::terminal::panel::stream(app, id, &update, tx),
                Event::AttachFailed(id, detail) => {
                    app.widgets.terminal.errors.insert(id, detail);
                }
                Event::ResizeLease(id) => {
                    app.widgets.terminal.leases.insert(id);
                }
            }
        });
    }

    pub(super) async fn terminal(&self, command: Command) -> viz_client::Result<()> {
        let result = match &command {
            Command::List => self
                .client
                .request(&ListTerminals::default())
                .await
                .map(|list| {
                    self.terminal_event(Event::Dock(TerminalMsg::Listed(list.terminals)));
                }),
            Command::Create(params, place) => self.client.request(params).await.map(|summary| {
                self.terminal_event(Event::Dock(TerminalMsg::Created(summary, *place)));
            }),
            Command::Attach(id) => {
                self.client
                    .request(&AttachTerminal { id: *id })
                    .await
                    .map(|snapshot| {
                        self.terminal_event(Event::Snapshot(*id, snapshot));
                    })
            }
            Command::Detach(id) => self
                .client
                .request(&DetachTerminal { id: *id })
                .await
                .map(drop),
            Command::Input(id, bytes) => self
                .client
                .request(&TerminalInput {
                    id: *id,
                    bytes: bytes.clone(),
                })
                .await
                .map(drop),
            Command::Resize(id, size, cell) => self
                .client
                .request(&ResizeTerminal {
                    id: *id,
                    size: *size,
                    cell: *cell,
                })
                .await
                .map(drop),
            Command::Close(id) => self
                .client
                .request(&CloseTerminal { id: *id })
                .await
                .map(drop),
        };
        if let Err(error) = result {
            if matches!(error, Error::Closed) {
                return Err(error);
            }
            match command {
                Command::Create(..) => {
                    self.terminal_event(Event::Dock(TerminalMsg::CreateFailed));
                }
                Command::Attach(id) => {
                    self.terminal_event(Event::AttachFailed(id, error.to_string()));
                    return Ok(());
                }
                Command::Resize(id, ..)
                    if matches!(
                        error,
                        Error::Rpc {
                            terminal_failure: Some(TerminalFailure::ResizeLease),
                            ..
                        }
                    ) =>
                {
                    self.terminal_event(Event::ResizeLease(id));
                    return Ok(());
                }
                _ => {}
            }
            toast(self.ui, "Terminal request failed", error.to_string());
        }
        Ok(())
    }
}
