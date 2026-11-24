//! Pure transitions for timeline disclosure, history pagination, subagent
//! links, fork/edit/reply actions, chunk loading, scroll state, and
//! running tool timers.

use viz_client::protocol::{
    EntryId, EntrySource, ForkPoint, ForkThread, Forked, SendInput, ThreadId,
};

use super::selection::{focus_draft, open_in_environment, open_thread, select_thread};
use crate::{
    effects::{EditSync, EditTarget, Effect, ServerEffect},
    model::{Item, LoadedEntry, Model},
};

/// Messages emitted by timeline rows, folds, inspectors, and cards.
#[derive(Debug)]
pub enum TimelineMsg {
    /// Cancel a running or queued delegated task.
    CancelTask(String),
    /// Toggle a per-thread timeline fold (turn fold, tool inspector, long
    /// message, plan collapse, or changed-files folder) anchored on screen.
    ToggleFold {
        /// Primary stable disclosure key.
        key: String,
        /// Optional legacy `-activity` key checked for scene compatibility.
        legacy_key: Option<String>,
        /// Default expanded state when no override is stored.
        default_open: bool,
        /// Optional header element key kept stationary under the pointer.
        anchor: Option<String>,
    },
    /// Expand or collapse every folder in a `ChangedFilesTree` card at once.
    ToggleChangedFolders {
        /// Folder fold keys inside the card.
        keys: Vec<String>,
        /// Target open state.
        expand: bool,
    },
    /// Toggle soft word wrapping on a Markdown code block.
    ToggleCodeWrap(String),
    /// Open or close a plan card's overflow action menu.
    TogglePlanMenu(String),
    /// Close any open plan card overflow menu.
    ClosePlanMenu,
    /// Viewport crossed the far-from-bottom threshold (`> 120pt`).
    SetFarFromBottom(bool),
    /// Glide the timeline viewport to its bottom edge.
    ScrollToEnd,
    /// Fetch the previous page of older transcript entries.
    LoadEarlier,
    /// Lazily load a truncated or omitted entry via `thread.entry_chunk`.
    LoadEntryChunk {
        /// Stable item key used to track loading state.
        key: String,
        /// Stored entry id or live entry key.
        source: EntrySource,
    },
    /// Fork the active thread before a user prompt (optionally loading it into
    /// the composer for editing).
    ForkBefore {
        /// Entry id of the user prompt.
        entry: EntryId,
        /// Load the excluded prompt into the new thread's composer draft.
        load_prompt: bool,
    },
    /// Fork the active thread through an assistant turn boundary.
    ForkThrough(EntryId),
    /// Quote an assistant response excerpt into the composer draft.
    ReplyTo(String),
    /// Hook for restoring a Git checkpoint prior to a user prompt (wired by
    /// Track G).
    RestoreCheckpoint(EntryId),
    /// Open the docked Diff panel, optionally focused on `path` (seam for Track
    /// G).
    OpenDiff {
        /// Optional relative file path to reveal in the Diff panel.
        path: Option<String>,
    },
    /// Open a delegated child or parent thread (local or remote).
    OpenThreadLink {
        /// Target thread id.
        thread: ThreadId,
        /// Optional remote environment id.
        environment: Option<String>,
    },
    /// Copy plain text to the clipboard.
    CopyText(String),
    /// Copy plain text and mark `key` for Copy -> Check icon morph feedback.
    CopyWithFeedback {
        /// Control key receiving copied feedback.
        key: String,
        /// Text placed on the clipboard.
        text: String,
    },
    /// Open an external link URL.
    OpenUrl(String),
    /// Dismiss an inline error banner by key.
    DismissError(String),
    /// Retry the most recent user prompt in the active thread.
    RetryLastPrompt,
    /// Advance the wall-clock timestamp used for live working timers and
    /// running tool timeout rings.
    TickToolClock(i64),
}

#[expect(
    clippy::too_many_lines,
    reason = "single exhaustive handler for all timeline interaction messages"
)]
pub(super) fn update(model: &mut Model, msg: TimelineMsg) -> Vec<Effect> {
    let mut effects = Vec::new();
    match msg {
        TimelineMsg::CancelTask(id) => effects.push(Effect::Server(ServerEffect::CancelTask(id))),
        TimelineMsg::ToggleFold {
            key,
            legacy_key,
            default_open,
            anchor,
        } => {
            let thread = model.selection.selected;
            let current = model.timeline.is_open(
                thread,
                &model.disclosure.expanded,
                &key,
                legacy_key.as_deref(),
                default_open,
            );
            model.timeline.set_fold(thread, key, !current);
            if let Some(anchor) = anchor {
                effects.push(Effect::KeepOnScreen(anchor));
            }
        }
        TimelineMsg::ToggleChangedFolders { keys, expand } => {
            let thread = model.selection.selected;
            for key in keys {
                model.timeline.set_fold(thread, key, expand);
            }
        }
        TimelineMsg::ToggleCodeWrap(key) => {
            if !model.timeline.wrapped_code.remove(&key) {
                model.timeline.wrapped_code.insert(key);
            }
        }
        TimelineMsg::TogglePlanMenu(key) => {
            if model.timeline.open_plan_menu.as_deref() == Some(key.as_str()) {
                model.timeline.open_plan_menu = None;
            } else {
                model.timeline.open_plan_menu = Some(key);
            }
        }
        TimelineMsg::ClosePlanMenu => {
            model.timeline.open_plan_menu = None;
        }
        TimelineMsg::SetFarFromBottom(far) => {
            model.timeline.far_from_bottom = far;
            if !far {
                model.timeline.unseen_activity = false;
            }
        }
        TimelineMsg::ScrollToEnd => {
            model.timeline.far_from_bottom = false;
            model.timeline.unseen_activity = false;
            effects.push(Effect::ScrollToBottom(model.timeline_page_key()));
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
        TimelineMsg::LoadEntryChunk { key, source } => {
            if model.connected()
                && let Some(thread) = model.selection.selected
                && model.timeline.loading_chunks.insert(key.clone())
            {
                effects.push(Effect::Server(ServerEffect::ReadEntryChunk {
                    thread,
                    key,
                    source,
                }));
            }
        }
        TimelineMsg::ForkBefore { entry, load_prompt } => {
            if model.connected()
                && let Some(thread) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::Fork {
                    params: ForkThread {
                        thread,
                        point: ForkPoint::Before { entry },
                        worktree: None,
                        harness: None,
                        title: None,
                        request_id: None,
                    },
                    load_prompt,
                }));
            }
        }
        TimelineMsg::ForkThrough(entry) => {
            if model.connected()
                && let Some(thread) = model.selection.selected
            {
                effects.push(Effect::Server(ServerEffect::Fork {
                    params: ForkThread {
                        thread,
                        point: ForkPoint::Through { entry },
                        worktree: None,
                        harness: None,
                        title: None,
                        request_id: None,
                    },
                    load_prompt: false,
                }));
            }
        }
        TimelineMsg::ReplyTo(source) => {
            let quoted = quote_excerpt(&source);
            if !quoted.is_empty() {
                let next = if model.composer.draft.text.trim().is_empty() {
                    format!("{quoted}\n\n")
                } else {
                    format!("{quoted}\n\n{}", model.composer.draft.text)
                };
                model.composer.draft.text.clone_from(&next);
                effects.push(Effect::SyncEdit(EditSync::Replace {
                    target: EditTarget::Draft,
                    text: next,
                    select_all: false,
                }));
                effects.push(focus_draft());
            }
        }
        TimelineMsg::RestoreCheckpoint(_entry) => {
            if model.git.status.is_some() {
                effects.extend(super::git::update(
                    model,
                    super::GitMsg::Dialog(Some(crate::model::git::Dialog::Restore)),
                ));
            } else {
                effects.push(Effect::Toast {
                    title: "Checkpoint restore requires a Git repository".into(),
                    detail: None,
                });
            }
        }
        TimelineMsg::OpenDiff { path } => {
            model.timeline.selected_diff_file.clone_from(&path);
            model.panels.diff.open = true;
            model.panels.details_open = false;
            model.git.popup = None;
            effects.push(Effect::SyncDiffPane {
                open: Some(true),
                width: None,
            });
            if model.git.status.is_some() {
                effects.extend(super::git::update(model, super::GitMsg::RefreshDiff));
            }
            if let Some(path) = path {
                effects.push(Effect::RevealDiffFile(path));
            }
        }
        TimelineMsg::OpenThreadLink {
            thread,
            environment,
        } => {
            model.git.popup = None;
            if let Some(environment) = environment {
                open_in_environment(model, environment, Some(thread), &mut effects);
            } else {
                select_thread(model, thread, &mut effects);
            }
        }
        TimelineMsg::CopyText(text) => effects.push(Effect::CopyText(text)),
        TimelineMsg::CopyWithFeedback { key, text } => {
            model.timeline.copied_key = Some(key);
            effects.push(Effect::CopyText(text));
        }
        TimelineMsg::OpenUrl(url) => effects.push(Effect::OpenUrl(url)),
        TimelineMsg::DismissError(key) => {
            model.timeline.dismissed_errors.insert(key);
        }
        TimelineMsg::RetryLastPrompt => {
            if model.connected()
                && !model.busy()
                && let Some(thread) = model.selection.selected
                && let Some(last_prompt) =
                    model
                        .transcript()
                        .items
                        .iter()
                        .rev()
                        .find_map(|item| match item {
                            Item::User(user) if !user.text.trim().is_empty() => {
                                Some(user.text.clone())
                            }
                            _ => None,
                        })
            {
                effects.push(Effect::Server(ServerEffect::SendInput {
                    recovery: None,
                    input: SendInput {
                        thread,
                        mode: None,
                        text: last_prompt,
                        request_id: None,
                        origin: None,
                        attachments: Vec::new(),
                        stored: Vec::new(),
                        context: Vec::new(),
                    },
                }));
            }
        }
        TimelineMsg::TickToolClock(now) => {
            model.disclosure.tool_clock = now;
        }
    }
    effects
}

fn quote_excerpt(source: &str) -> String {
    let lines: Vec<String> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|line| {
            let clipped = match line.char_indices().nth(120) {
                Some((end, _)) => format!("{}…", &line[..end]),
                None => line.to_owned(),
            };
            format!("> {clipped}")
        })
        .collect();
    lines.join("\n")
}

pub(super) fn apply_thread_forked(
    model: &mut Model,
    forked: Forked,
    load_prompt: bool,
    effects: &mut Vec<Effect>,
) {
    let Forked { thread, prompt } = forked;
    open_thread(model, thread, effects);
    if load_prompt && let Some(prompt) = prompt {
        model.composer.draft.text.clone_from(&prompt.text);
        effects.push(Effect::SyncEdit(EditSync::Replace {
            target: EditTarget::Draft,
            text: prompt.text,
            select_all: false,
        }));
        effects.push(focus_draft());
    }
}

pub(super) fn apply_entry_chunk_loaded(
    model: &mut Model,
    thread: ThreadId,
    key: &str,
    source: &EntrySource,
    result: Result<LoadedEntry, String>,
    effects: &mut Vec<Effect>,
) {
    model.timeline.loading_chunks.remove(key);
    match result {
        Ok(loaded) => {
            if let Some(open) = model.cache_mut().open_threads.get_mut(&thread) {
                open.transcript.apply_loaded_entry(source, loaded);
            }
        }
        Err(error) => {
            effects.push(Effect::Toast {
                title: "Couldn’t load full output".into(),
                detail: Some(error),
            });
        }
    }
}
