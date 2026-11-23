//! Pure transitions for timeline disclosure, history pagination, subagent
//! links, clipboard copy, and running tool timers.

use viz_client::protocol::ThreadId;

use super::selection::{open_in_environment, select_thread};
use crate::{
    effects::{Effect, ServerEffect},
    model::Model,
};

/// Messages emitted by timeline rows and headers.
#[derive(Debug)]
pub enum TimelineMsg {
    /// Expand or collapse a timeline row or tucked deck by key.
    ToggleExpanded(String),
    /// Fetch the previous page of older transcript entries.
    LoadEarlier,
    /// Open a delegated child or parent thread (local or remote).
    OpenThreadLink {
        /// Target thread id.
        thread: ThreadId,
        /// Optional remote environment id.
        environment: Option<String>,
    },
    /// Copy text to the system clipboard.
    CopyText(String),
    /// Open an external link URL.
    OpenUrl(String),
    /// Advance the wall-clock timestamp used for running tool timeout rings.
    TickToolClock(i64),
}

pub(super) fn update(model: &mut Model, msg: TimelineMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        TimelineMsg::ToggleExpanded(key) => {
            if !model.disclosure.expanded.remove(&key) {
                model.disclosure.expanded.insert(key);
            }
        }
        TimelineMsg::LoadEarlier => {
            if !model.is_history_loading()
                && model.connected()
                && let (Some(thread), Some(before)) =
                    (model.selection.selected, model.transcript().before)
            {
                model.cache_mut().open_thread_mut(thread).history_loading = true;
                effects.push(Effect::Server(ServerEffect::LoadEarlier { thread, before }));
            }
        }
        TimelineMsg::OpenThreadLink {
            thread,
            environment,
        } => {
            if let Some(environment) = environment {
                open_in_environment(model, environment, Some(thread), &mut effects);
            } else {
                select_thread(model, thread, &mut effects);
            }
        }
        TimelineMsg::CopyText(text) => effects.push(Effect::CopyText(text)),
        TimelineMsg::OpenUrl(url) => effects.push(Effect::OpenUrl(url)),
        TimelineMsg::TickToolClock(now) => {
            model.disclosure.tool_clock = now;
        }
    }
    effects
}
