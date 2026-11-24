//! Pure transitions for timeline disclosure, history pagination, subagent
//! links, fork/edit/reply actions, chunk loading, scroll state, and
//! running tool timers.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use viz_client::protocol::{
    EntryId, EntrySource, FileRoot, ForkPoint, ForkThread, Forked, LineWindow, SendInput, ThreadId,
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
    /// Restore a Git checkpoint prior to a user prompt.
    RestoreCheckpoint(EntryId),
    /// Open the docked Diff panel, optionally focused on `path`.
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
    /// Open a link URL (`file://` and `viz-attachment://` open the in-app
    /// preview modal; web URLs open externally).
    OpenUrl(String),
    /// Always open a URL in the OS external handler.
    OpenExternalUrl(String),
    /// Open a workspace or absolute file in the in-app file/image preview
    /// modal, optionally highlighting an inclusive 1-based line range.
    OpenFilePreview {
        /// Absolute or workspace-relative file path.
        path: String,
        /// Optional inclusive 1-based line range (`start..=end`).
        lines: Option<(usize, usize)>,
    },
    /// Open a stored thread attachment in the preview modal / image lightbox.
    OpenAttachmentPreview {
        /// Owning thread id.
        thread: ThreadId,
        /// Attachment SHA-256 id.
        id: String,
        /// Display filename.
        name: String,
        /// MIME media type.
        mime: String,
    },
    /// Close any open file or attachment preview modal.
    CloseFilePreview,
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
            default_open,
            anchor,
        } => {
            let thread = model.selection.selected;
            let current = model.timeline.is_open(thread, &key, default_open);
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
        TimelineMsg::OpenUrl(url) => {
            if let Some((path, lines)) = crate::timeline::file_preview::parse_file_link(&url) {
                open_file_preview(model, path, lines, Some(url), &mut effects);
            } else if let Some((thread, id)) =
                crate::timeline::file_preview::parse_attachment_link(&url)
            {
                let (name, mime) = lookup_attachment_meta(model, thread, &id);
                open_attachment_preview(model, thread, id, name, &mime, &mut effects);
            } else {
                effects.push(Effect::OpenUrl(url));
            }
        }
        TimelineMsg::OpenExternalUrl(url) => effects.push(Effect::OpenUrl(url)),
        TimelineMsg::OpenFilePreview { path, lines } => {
            let open_url = crate::timeline::file_preview::path_to_file_url(&path);
            open_file_preview(model, path, lines, open_url, &mut effects);
        }
        TimelineMsg::OpenAttachmentPreview {
            thread,
            id,
            name,
            mime,
        } => open_attachment_preview(model, thread, id, name, &mime, &mut effects),
        TimelineMsg::CloseFilePreview => {
            model.timeline.file_preview = None;
            effects.push(Effect::ClearPreviewImage);
        }
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

fn open_file_preview(
    model: &mut Model,
    path: String,
    lines: Option<(usize, usize)>,
    open_url: Option<String>,
    effects: &mut Vec<Effect>,
) {
    model.timeline.file_preview_seq = model.timeline.file_preview_seq.wrapping_add(1);
    let request = model.timeline.file_preview_seq;
    effects.push(Effect::ClearPreviewImage);
    if !model.connected() {
        model.timeline.file_preview = Some(crate::model::FilePreview {
            request,
            path,
            lines,
            open_url,
            body: crate::model::FilePreviewBody::Error("Server is offline.".into()),
        });
        return;
    }
    let root_and_base = model.selection.selected.map_or_else(
        || {
            model.project_path().map(|project| {
                (
                    FileRoot::Project {
                        project: project.to_owned(),
                    },
                    Some(project),
                )
            })
        },
        |thread| {
            let base = model.thread_state().map(|st| st.cwd.as_str());
            Some((FileRoot::Thread { thread }, base))
        },
    );
    let Some((root, base)) = root_and_base else {
        model.timeline.file_preview = Some(crate::model::FilePreview {
            request,
            path,
            lines,
            open_url,
            body: crate::model::FilePreviewBody::Error(
                "Select a thread or project to preview files.".into(),
            ),
        });
        return;
    };
    let Some(rel_path) = workspace_relative_path(base, &path) else {
        model.timeline.file_preview = Some(crate::model::FilePreview {
            request,
            path,
            lines,
            open_url,
            body: crate::model::FilePreviewBody::Error(
                "File is outside the active workspace.".into(),
            ),
        });
        return;
    };
    let open_url = open_url.or_else(|| {
        base.and_then(|dir| {
            let full = std::path::Path::new(dir).join(&rel_path);
            crate::timeline::file_preview::path_to_file_url(&full.to_string_lossy())
        })
    });
    let window = lines.map(|(start, end)| LineWindow {
        start: u32::try_from(start.saturating_sub(200).max(1)).unwrap_or(1),
        count: u32::try_from(
            end.saturating_sub(start)
                .saturating_add(400)
                .clamp(400, 1_200),
        )
        .unwrap_or(600),
    });
    model.timeline.file_preview = Some(crate::model::FilePreview {
        request,
        path: rel_path.clone(),
        lines,
        open_url,
        body: crate::model::FilePreviewBody::Loading,
    });
    effects.push(Effect::Server(ServerEffect::ReadFile {
        request,
        root,
        path: rel_path,
        window,
    }));
}

fn workspace_relative_path(base: Option<&str>, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = std::path::Path::new(trimmed);
    if path.is_absolute() {
        let root = std::path::Path::new(base?);
        let stripped = path.strip_prefix(root).ok()?;
        let rel = stripped.to_str()?;
        (!rel.is_empty()).then(|| rel.to_owned())
    } else {
        let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
        (!stripped.is_empty()).then(|| stripped.to_owned())
    }
}

fn open_attachment_preview(
    model: &mut Model,
    thread: ThreadId,
    id: String,
    name: String,
    mime: &str,
    effects: &mut Vec<Effect>,
) {
    model.timeline.file_preview_seq = model.timeline.file_preview_seq.wrapping_add(1);
    let request = model.timeline.file_preview_seq;
    effects.push(Effect::ClearPreviewImage);
    model.timeline.file_preview = Some(crate::model::FilePreview {
        request,
        path: name,
        lines: None,
        open_url: None,
        body: if mime.starts_with("image/") {
            crate::model::FilePreviewBody::Image { size: 0 }
        } else {
            crate::model::FilePreviewBody::Loading
        },
    });
    let key = (thread, id.clone());
    if let Some(cached) = model.timeline.loaded_attachments.get(&key).cloned() {
        apply_loaded_attachment_to_preview(model, request, &cached, effects);
        return;
    }
    if !model.connected() {
        if let Some(preview) = model.timeline.file_preview.as_mut() {
            preview.body = crate::model::FilePreviewBody::Error("Server is offline.".into());
        }
        return;
    }
    model.timeline.pending_attachments.insert(key);
    effects.push(Effect::Server(ServerEffect::ReadAttachment {
        thread,
        id,
        preview_request: Some(request),
    }));
}

pub(super) fn apply_loaded_attachment_to_preview(
    model: &mut Model,
    request: u64,
    attachment: &viz_client::protocol::Attachment,
    effects: &mut Vec<Effect>,
) {
    let Some(preview) = model.timeline.file_preview.as_mut() else {
        return;
    };
    if preview.request != request {
        return;
    }
    let Ok(bytes) = STANDARD.decode(&attachment.data) else {
        preview.body =
            crate::model::FilePreviewBody::Error("Couldn’t decode attachment bytes".into());
        return;
    };
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if !attachment.name.is_empty() {
        preview.path.clone_from(&attachment.name);
    }
    if attachment.mime.starts_with("image/") {
        preview.body = crate::model::FilePreviewBody::Image { size };
        effects.push(Effect::DecodePreviewImage {
            media_type: attachment.mime.clone(),
            bytes,
        });
    } else if let Ok(text) = String::from_utf8(bytes) {
        preview.body = crate::model::FilePreviewBody::Text {
            text,
            start_line: 1,
            truncated: false,
            size,
        };
    } else {
        preview.body = crate::model::FilePreviewBody::Binary {
            mime: attachment.mime.clone(),
            size,
        };
    }
}

fn lookup_attachment_meta(model: &Model, thread: ThreadId, id: &str) -> (String, String) {
    if let Some(cached) = model
        .timeline
        .loaded_attachments
        .get(&(thread, id.to_owned()))
    {
        return (cached.name.clone(), cached.mime.clone());
    }
    if let Some(open) = model.cache().open_threads.get(&thread) {
        for item in &open.transcript.items {
            if let Item::User(user) = item
                && let Some(att) = user.attachments.iter().find(|a| a.id == id)
            {
                return (att.name.clone(), att.mime.clone());
            }
        }
    }
    (id.to_owned(), "application/octet-stream".into())
}
