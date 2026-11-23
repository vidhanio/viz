//! Connection-local terminal subscriptions and resize leases.

use std::{collections::HashMap, future, sync::Arc};

use futures_util::{
    StreamExt as _,
    stream::{self, BoxStream},
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::{StreamMap, wrappers::BroadcastStream};
use viz_protocol::{
    AttachTerminal, DetachTerminal, Empty, Method, Notification, ResizeTerminal, TerminalId, Update,
};
use viz_terminal::pty::{Attachment, PtyError};

use super::{
    Server,
    socket::{json_message, send_note},
};
use crate::{
    error::Error,
    rpc,
    wire::{Request, Response, params},
};

/// The end marker lets the socket release the attachment even on a quiet PTY.
pub(super) enum Event {
    Update(Arc<Update>),
    Lagged,
    Closed,
}

pub(super) enum WatchEvent {
    Stream(TerminalId, Event),
    Changed,
}

pub(super) struct TerminalWatches {
    changes: broadcast::Receiver<()>,
    attachments: HashMap<TerminalId, Attachment>,
    streams: StreamMap<TerminalId, BoxStream<'static, Event>>,
}

impl TerminalWatches {
    pub(super) fn new(changes: broadcast::Receiver<()>) -> Self {
        Self {
            changes,
            attachments: HashMap::new(),
            streams: StreamMap::new(),
        }
    }

    pub(super) async fn request(
        &mut self,
        server: &Server,
        request: Request<'_>,
    ) -> serde_json::Result<axum::extract::ws::Message> {
        let id = request.id.clone();
        match request.method_name() {
            AttachTerminal::NAME => {
                let result = async {
                    let AttachTerminal { id } = params(&request)?;
                    let (snapshot, updates) = if let Some(attachment) = self.attachments.get(&id) {
                        attachment.refresh().await?
                    } else {
                        let handle = server.workspace.terminal(id).await?;
                        let (stream, attachment) = handle.attach().await?;
                        self.attachments.insert(id, attachment);
                        stream
                    };
                    self.stream(id, updates);
                    Ok(snapshot)
                }
                .await;
                json_message(&Response::new(id, result))
            }
            DetachTerminal::NAME => {
                let result = params::<DetachTerminal>(&request).map(|args| {
                    self.detach(args.id);
                    Empty {}
                });
                json_message(&Response::new(id, result))
            }
            ResizeTerminal::NAME => {
                let result = async {
                    let ResizeTerminal { id, size, cell } = params(&request)?;
                    self.attachments
                        .get(&id)
                        .ok_or(Error::TerminalNotAttached)?
                        .resize(size, cell)
                        .await?;
                    Ok(Empty {})
                }
                .await;
                json_message(&Response::new(id, result))
            }
            method => json_message(&Response::<Empty>::new(id, Err(rpc::unknown(method)))),
        }
    }

    /// Only polls for the next event, so a competing RPC cannot cancel a send
    /// or discard a consumed update inside the socket's select.
    pub(super) async fn next(&mut self) -> Option<WatchEvent> {
        tokio::select! {
            event = self.streams.next(), if !self.streams.is_empty() => {
                event.map(|(id, event)| WatchEvent::Stream(id, event))
            },
            change = self.changes.recv() => match change {
                Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => Some(WatchEvent::Changed),
                Err(broadcast::error::RecvError::Closed) => None,
            },
        }
    }

    pub(super) async fn forward(
        &mut self,
        event: WatchEvent,
        outgoing: &mpsc::Sender<axum::extract::ws::Message>,
    ) -> anyhow::Result<()> {
        let WatchEvent::Stream(id, event) = event else {
            return send_note(outgoing, Notification::TerminalsChanged(Empty {})).await;
        };
        let note = match event {
            Event::Update(update) => Notification::TerminalUpdate {
                id,
                update: (*update).clone(),
            },
            Event::Lagged => {
                let attachment = &self.attachments[&id];
                match attachment.refresh().await {
                    Ok((snapshot, updates)) => {
                        self.stream(id, updates);
                        Notification::TerminalSnapshot { id, snapshot }
                    }
                    Err(PtyError::Closed) => {
                        self.detach(id);
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Event::Closed => {
                self.detach(id);
                return Ok(());
            }
        };
        send_note(outgoing, note).await
    }

    fn stream(&mut self, id: TerminalId, updates: broadcast::Receiver<Arc<Update>>) {
        self.streams.insert(
            id,
            BroadcastStream::new(updates)
                .map(|result| result.map_or(Event::Lagged, Event::Update))
                .chain(stream::once(future::ready(Event::Closed)))
                .boxed(),
        );
    }

    fn detach(&mut self, id: TerminalId) {
        self.streams.remove(&id);
        self.attachments.remove(&id);
    }
}
