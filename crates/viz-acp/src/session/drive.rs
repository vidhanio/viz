//! Drives a prompt and publishes its batched and final transcript updates.

use std::{future::Future, mem, sync::Arc, time::Instant};

use tokio::{sync::mpsc, time::sleep_until};
use tokio_util::sync::CancellationToken;
use viz_protocol::{ContextUsage, RunId, TurnTokens, TurnUsage, UsageCost};

use super::{Session, SessionSnapshot, state::lock};
use crate::{
    Body, Client, Error, LiveTurn, ObservedUpdate, Output, Receipts, StoredAttachment, ToolCall,
    Transcript,
    error::Result,
    prompt::{build_prompt, load_attachments},
    schema::{ContentBlock, PromptRequest, PromptResponse, StopReason},
    turn::{Action, Signal, Turn},
};

/// One prompt turn's input.
#[derive(Debug)]
pub struct Prompt<'a> {
    pub text: &'a str,
    /// The live turn's identity; text run keys extend it.
    pub turn: String,
    /// The run the turn belongs to: the scope of the MCP tools it calls.
    pub run: RunId,
    pub context: &'a [ContentBlock],
    pub attachments: &'a [StoredAttachment],
    /// Cancelling sends `session/cancel` at once; the turn then ends
    /// `cancelled`.
    pub cancel: CancellationToken,
    /// Steering waits until no tool calls are open before sending
    /// `session/cancel`, so tool execution finishes cleanly before
    /// interjecting.
    pub steer: CancellationToken,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The agent's message text.
    pub answer: String,
    /// The ACP stop reason, such as `end_turn` or `cancelled`.
    pub stop_reason: StopReason,
    /// Per-turn token usage, when reported with the response.
    pub usage: Option<TurnUsage>,
}

impl Session {
    /// Runs one prompt turn. `updates` receives the entries the turn commits,
    /// in order, with the live tail that follows them and a snapshot when
    /// session state changed since its previous call; calls are serial, batched
    /// about every 75 ms, and the last one has no live tail. It is called for
    /// the end of the turn even after an earlier call failed. ACP prompts have
    /// no idempotency key, so a turn is never retried.
    ///
    /// Sends a prompt and streams its updates until the turn settles, fails, or
    /// is cancelled/steered.
    ///
    /// # Errors
    ///
    /// When the turn fails, the harness closes, or `updates` fails; the session
    /// is then no longer alive.
    pub async fn prompt<F, Fut, E>(&mut self, input: Prompt<'_>, mut updates: F) -> Result<Outcome>
    where
        F: FnMut(Vec<Output>, Option<LiveTurn>, Option<SessionSnapshot>) -> Fut + Send,
        Fut: Future<Output = std::result::Result<(), E>> + Send,
        E: std::error::Error + Send + Sync + 'static,
    {
        if !self.is_alive() {
            return Err(Error::Closed(
                "the session cannot take another turn".to_owned(),
            ));
        }
        if input.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let bytes = load_attachments(input.attachments, self.agent.capabilities()).await?;
        let attachments: Vec<_> = input
            .attachments
            .iter()
            .zip(bytes.iter().map(Option::as_deref))
            .collect();
        let text = self.handoff.as_ref().map(|history| format!(
            "Prior conversation for context. Treat it as history and answer only the new request.\n\n{history}\n\nNew request:\n{}",
            input.text,
        ));
        let blocks = build_prompt(
            text.as_deref().unwrap_or(input.text),
            input.context,
            &attachments,
            self.agent.capabilities(),
        )?;
        if input.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        self.broken = true;
        let (outcome, settled) = self.drive_turn(input, blocks, &mut updates).await?;
        if outcome.stop_reason != StopReason::Cancelled {
            self.handoff = None;
        }
        self.broken = !settled || self.agent.is_closed();
        Ok(outcome)
    }

    async fn drive_turn<F, Fut, E>(
        &mut self,
        input: Prompt<'_>,
        blocks: Vec<ContentBlock>,
        updates: &mut F,
    ) -> Result<(Outcome, bool)>
    where
        F: FnMut(Vec<Output>, Option<LiveTurn>, Option<SessionSnapshot>) -> Fut + Send,
        Fut: Future<Output = std::result::Result<(), E>> + Send,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.prompted = true;
        let receipts = Arc::new(Receipts::default());
        let mut transcript = Transcript::new(input.turn, Arc::clone(&receipts));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut pending = self.begin_prompt(sender);
        let tools = self
            .mcp
            .as_ref()
            .map(|endpoint| endpoint.activate(input.run, input.cancel.child_token(), receipts));
        let state = Arc::clone(&self.state);
        let response = self.agent.prompt_with_boundary(
            PromptRequest::new(self.id.clone(), blocks),
            move || {
                lock(&state).turn = None;
            },
        );
        tokio::pin!(response);

        let mut guard = ActivePrompt {
            agent: &self.agent,
            id: &self.id,
            settled: false,
        };
        let mut turn = Turn::new(Instant::now(), !pending.is_empty());
        let result = loop {
            let signal = tokio::select! {
                biased;
                result = &mut response => {
                    guard.settled = true;
                    break result;
                }
                () = input.cancel.cancelled(), if !turn.cancelling() => Signal::Cancel,
                () = wait_until(turn.cancel_deadline()) => Signal::CancelExpired,
                () = input.steer.cancelled(), if turn.accepts_steer() => Signal::Steer,
                () = wait_until(turn.batch_deadline()) => Signal::BatchReady,
                Some(update) = receiver.recv() => {
                    pending.push(update);
                    Signal::Update
                }
            };
            match turn.advance(signal, Instant::now()) {
                Action::Wait => {}
                Action::Cancel => {
                    let _ = self.agent.cancel(&self.id);
                }
                Action::Timeout => break Err(Error::CancelTimeout),
                Action::Publish => {
                    drain_updates(&mut receiver, &mut pending);
                    if !pending.is_empty() {
                        let mut entries = transcript.apply(mem::take(&mut pending));
                        self.fill_terminals(&mut entries);
                        let mut live = transcript.live();
                        self.fill_live_terminals(&mut live);
                        if let Err(error) = updates(entries, Some(live), self.take_snapshot()).await
                        {
                            break Err(Error::Publish(Box::new(error)));
                        }
                    }
                    if matches!(
                        turn.after_publish(transcript.has_open_tools(), Instant::now()),
                        Action::Cancel
                    ) {
                        let _ = self.agent.cancel(&self.id);
                    }
                }
            }
        };
        let settled = guard.settled;
        drop(guard);
        drop(tools);
        let usage = {
            let mut state = lock(&self.state);
            state.turn = None;
            state.turn_usage.take()
        };
        drain_updates(&mut receiver, &mut pending);
        let mut entries = transcript.apply(pending);
        entries.extend(transcript.finish());
        self.fill_terminals(&mut entries);
        let published = updates(entries, None, self.take_snapshot())
            .await
            .map_err(|error| Error::Publish(Box::new(error)));
        let response = match result {
            Ok(response) => response,
            Err(Error::CancelTimeout | Error::Closed(_)) if turn.cancelling() => {
                PromptResponse::new(StopReason::Cancelled)
            }
            Err(error) => return Err(crate::failure::classify(error, transcript.last_prose())),
        };
        published?;
        Ok((
            Outcome {
                answer: transcript.answer().to_owned(),
                stop_reason: response.stop_reason,
                usage: turn_usage(usage, response.usage),
            },
            settled,
        ))
    }

    fn fill_live_terminals(&self, live: &mut LiveTurn) {
        for entry in &mut live.entries {
            if let Body::Tool(tool) = &mut entry.body {
                self.fill_tool_terminals(tool);
            }
        }
    }

    fn fill_terminals(&self, entries: &mut [Output]) {
        for entry in entries {
            if let Output::Entry(entry) = entry
                && let Body::Tool(tool) = &mut entry.body
            {
                self.fill_tool_terminals(tool);
            }
        }
    }

    fn fill_tool_terminals(&self, tool: &mut ToolCall) {
        if tool.terminals.is_empty() {
            return;
        }
        let session = crate::schema::SessionId::new(self.id.clone());
        for terminal in &mut tool.terminals {
            if let Some(output) = self.agent.terminal_output(
                &session,
                &crate::schema::TerminalId::new(terminal.id.clone()),
            ) {
                crate::tool::host_output(terminal, output);
                if terminal.exit.is_some() {
                    tool.exit.clone_from(&terminal.exit);
                }
            }
        }
    }

    fn begin_prompt(
        &mut self,
        sender: mpsc::UnboundedSender<ObservedUpdate>,
    ) -> Vec<ObservedUpdate> {
        let mut state = lock(&self.state);
        state.turn = Some(sender);
        state.turn_usage = None;
        state.dirty = false;
        drop(state);
        self.idle_updates()
    }
}
struct ActivePrompt<'a> {
    agent: &'a Client,
    id: &'a str,
    settled: bool,
}

impl Drop for ActivePrompt<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.agent.cancel(self.id);
        }
    }
}

fn drain_updates(
    receiver: &mut mpsc::UnboundedReceiver<ObservedUpdate>,
    pending: &mut Vec<ObservedUpdate>,
) {
    let queued = receiver.len();
    pending.extend(std::iter::from_fn(|| receiver.try_recv().ok()).take(queued));
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}

/// The wire name of a stop reason, such as `end_turn` or `cancelled`.
#[must_use]
pub fn stop_reason_name(reason: StopReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{reason:?}"))
}

fn turn_usage(
    update: Option<crate::schema::UsageUpdate>,
    tokens: Option<crate::schema::Usage>,
) -> Option<TurnUsage> {
    if update.is_none() && tokens.is_none() {
        return None;
    }
    Some(TurnUsage {
        context: update.as_ref().map(|update| ContextUsage {
            used: update.used,
            size: update.size,
        }),
        cost: update.and_then(|update| update.cost).and_then(|cost| {
            Some(UsageCost {
                currency: cost.currency,
                amount: serde_json::Number::from_f64(cost.amount)?,
            })
        }),
        tokens: tokens.map(|tokens| TurnTokens {
            total_tokens: tokens.total_tokens,
            input_tokens: tokens.input_tokens,
            output_tokens: tokens.output_tokens,
            thought_tokens: tokens.thought_tokens,
            cached_read_tokens: tokens.cached_read_tokens,
            cached_write_tokens: tokens.cached_write_tokens,
        }),
    })
}
