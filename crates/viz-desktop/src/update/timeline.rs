//! Pure transitions for timeline disclosure, history pagination, subagent
//! links, fork/edit/reply actions, chunk loading, scroll state, and
//! running tool timers.

use viz_client::protocol::{
    EntryAnchor, EntryData, EntryId, FileRoot, ForkPoint, ForkThread, Forked, LineWindow,
    SendInput, ThreadId,
};

use super::selection::{focus_draft, open_in_environment, open_thread, select_thread};
use crate::{
    effects::{EditSync, Effect, ServerEffect},
    model::{EntryAction, FilePreview, FilePreviewBody, Item, Model, PreviewFile, PreviewLines},
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
    /// Fetch the next detached history page.
    LoadLater,
    /// Hydrate trimmed stored entries over the content plane.
    HydrateEntries(Vec<(String, EntryId)>),
    /// Copy the original prompt, hydrating it first when needed.
    CopyPrompt { key: String, entry: EntryId },
    /// View a full entry in the modal viewer.
    OpenEntryPreview {
        key: String,
        entry: EntryId,
        title: String,
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
    /// Open the active workspace file in the OS handler.
    OpenLocalFile(u64),
    /// Save the active workspace file from the remote server.
    DownloadFile(u64),
    /// Visible document rows in the active file preview.
    FilePreviewVisible {
        request: u64,
        rows: std::ops::Range<usize>,
    },
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
            let opening = !model.is_fold_open(&key, default_open);
            model.set_fold(model.selection.selected, key.clone(), opening);
            if opening {
                let target = key.strip_suffix("-full").unwrap_or(&key);
                if let Some((entry, full)) = model
                    .transcript()
                    .items
                    .iter()
                    .find(|item| item.key() == target)
                    .and_then(Item::partial_entry)
                    && full <= viz_client::protocol::limits::AUTO_HYDRATE_ENTRY_BYTES
                {
                    request_entry(model, target.to_owned(), entry, &mut effects);
                }
            }
            if let Some(anchor) = anchor {
                effects.push(Effect::KeepOnScreen(anchor));
            }
        }
        TimelineMsg::ToggleChangedFolders { keys, expand } => {
            let thread = model.selection.selected;
            for key in keys {
                model.set_fold(thread, key, expand);
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
            if let Some(thread) = model.selection.selected
                && model.detached()
            {
                let open = model.cache_mut().open_thread_mut(thread);
                open.history = None;
                open.history_request = None;
                model.timeline.bump_fold_rev();
            }
            model.timeline.far_from_bottom = false;
            model.timeline.unseen_activity = false;
            effects.push(Effect::ScrollToBottom(model.timeline_page_key()));
        }
        TimelineMsg::LoadEarlier | TimelineMsg::LoadLater => {
            if model.history_request().is_none()
                && model.connected()
                && let Some(thread) = model.selection.selected
            {
                let anchor = if matches!(msg, TimelineMsg::LoadEarlier) {
                    model
                        .transcript()
                        .before
                        .map(|entry| EntryAnchor::Before { entry })
                } else {
                    model
                        .open_thread()
                        .and_then(|open| open.history.as_ref())
                        .and_then(|window| window.after)
                        .map(|entry| EntryAnchor::After { entry })
                };
                if let Some(anchor) = anchor {
                    super::palette::request_history(model, thread, anchor, &mut effects);
                }
            }
        }
        TimelineMsg::HydrateEntries(requests) => {
            for (key, entry) in requests {
                request_entry(model, key, entry, &mut effects);
            }
        }
        TimelineMsg::CopyPrompt { key, entry } => {
            if let Some(thread) = model.selection.selected.filter(|_| model.connected()) {
                model
                    .timeline
                    .entry_actions
                    .insert((thread, entry), EntryAction::CopyPrompt);
                request_entry(model, key, entry, &mut effects);
            }
        }
        TimelineMsg::OpenEntryPreview { key, entry, title } => {
            open_entry_preview(model, key, entry, title, &mut effects);
        }
        TimelineMsg::ForkBefore { entry, load_prompt } => {
            if model.connected()
                && let Some(thread) = model.selection.selected
            {
                let trimmed_key = load_prompt
                    .then(|| {
                        model.transcript().items.iter().find_map(|item| match item {
                            Item::User(user)
                                if user.entry_id == Some(entry) && user.full.is_some() =>
                            {
                                Some(user.key.clone())
                            }
                            _ => None,
                        })
                    })
                    .flatten();
                if let Some(key) = trimmed_key {
                    model
                        .timeline
                        .entry_actions
                        .insert((thread, entry), EntryAction::EditPrompt);
                    request_entry(model, key, entry, &mut effects);
                } else {
                    fork_before(thread, entry, load_prompt, &mut effects);
                }
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
                let draft = &mut model.composer.draft;
                let prefix = format!("{quoted}\n\n");
                if draft.text.trim().is_empty() {
                    draft.text = prefix;
                    draft.pills.clear();
                } else {
                    for pill in &mut draft.pills {
                        pill.range.start += prefix.len();
                        pill.range.end += prefix.len();
                    }
                    draft.text.insert_str(0, &prefix);
                }
                effects.push(Effect::SyncEdit(EditSync::ReplaceDraftWithPills {
                    text: draft.text.clone(),
                    pills: draft.pills.clone(),
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
            copy_feedback(model, key, text, &mut effects);
        }
        TimelineMsg::OpenUrl(url) => {
            if let Some((path, lines)) = crate::timeline::file_preview::parse_file_link(&url) {
                open_file_preview(model, path, lines, &mut effects);
            } else if let Some((thread, id)) =
                crate::timeline::file_preview::parse_attachment_link(&url)
            {
                let (name, mime) = lookup_attachment_meta(model, thread, &id);
                open_attachment_preview(model, thread, id, name, &mime, &mut effects);
            } else {
                effects.push(Effect::OpenUrl(url));
            }
        }
        TimelineMsg::OpenLocalFile(request) => {
            if let Some(preview) = active_preview_file(model, request)
                && let Some(path) = preview
                    .source
                    .as_ref()
                    .and_then(|file| file.local_root.clone())
            {
                effects.push(Effect::Server(ServerEffect::OpenLocalFile {
                    directory: path,
                    path: preview.path.clone(),
                }));
            }
        }
        TimelineMsg::DownloadFile(request) => {
            if let Some(preview) = active_preview_file(model, request)
                && let Some(file) = &preview.source
                && file.local_root.is_none()
            {
                effects.push(Effect::Server(ServerEffect::DownloadFile {
                    root: file.root.clone(),
                    path: preview.path.clone(),
                }));
            }
        }
        TimelineMsg::FilePreviewVisible { request, rows } => {
            file_preview_visible(model, request, rows, &mut effects);
        }
        TimelineMsg::OpenFilePreview { path, lines } => {
            open_file_preview(model, path, lines, &mut effects);
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
            model.timeline.bump_fold_rev();
        }
        TimelineMsg::RetryLastPrompt => {
            if model.connected()
                && !model.busy()
                && let Some(thread) = model.selection.selected
                && let Some(Item::User(user)) = model.transcript().items.iter().rev().find(
                    |item| matches!(item, Item::User(user) if !user.prompt.text.trim().is_empty()),
                )
            {
                if let Some(entry) = user.entry_id.filter(|_| user.full.is_some()) {
                    let key = user.key.clone();
                    model
                        .timeline
                        .entry_actions
                        .insert((thread, entry), EntryAction::RetryPrompt);
                    request_entry(model, key, entry, &mut effects);
                } else {
                    retry_prompt(thread, user.prompt.clone(), &mut effects);
                }
            }
        }
        TimelineMsg::TickToolClock(now) => {
            model.disclosure.tool_clock = now;
        }
    }
    effects
}

fn retry_prompt(thread: ThreadId, prompt: viz_client::protocol::Prompt, effects: &mut Vec<Effect>) {
    effects.push(Effect::Server(ServerEffect::SendInput {
        recovery: None,
        input: SendInput {
            thread,
            mode: None,
            prompt,
            request_id: None,
            origin: None,
        },
    }));
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
        super::composer_actions::load_prompt(model, &prompt, effects);
        effects.push(focus_draft());
    }
}

pub(super) fn apply_entry_loaded(
    model: &mut Model,
    thread: ThreadId,
    key: &str,
    entry: EntryId,
    result: Result<Box<EntryData>, String>,
    effects: &mut Vec<Effect>,
) {
    let next = model
        .connection
        .live
        .finish_entry(thread, entry, model.selection.selected);
    let action = model.timeline.entry_actions.remove(&(thread, entry));
    let oversized = model.selection.selected == Some(thread)
        && model.transcript().items.iter().any(|item| {
            item.partial_entry().is_some_and(|(id, size)| {
                id == entry && size > viz_client::protocol::limits::AUTO_HYDRATE_ENTRY_BYTES
            })
        });
    match result {
        Ok(loaded) => {
            if let Some(EntryAction::Preview(request)) = action {
                if let Some(preview) = model
                    .timeline
                    .file_preview
                    .as_mut()
                    .filter(|p| p.request == request)
                {
                    preview.body = entry_preview_text(&loaded).map_or_else(
                        || FilePreviewBody::Error("The entry has no displayable output.".into()),
                        FilePreviewBody::entry_text,
                    );
                }
            } else {
                if let EntryData::Prompt { prompt, .. } = loaded.as_ref() {
                    match action {
                        Some(EntryAction::CopyPrompt) => copy_feedback(
                            model,
                            format!("copy-{key}"),
                            prompt.display_text(),
                            effects,
                        ),
                        Some(EntryAction::EditPrompt) => fork_before(thread, entry, true, effects),
                        Some(EntryAction::RetryPrompt)
                            if model.selection.selected == Some(thread) && !model.busy() =>
                        {
                            retry_prompt(thread, prompt.clone(), effects);
                        }
                        _ => {}
                    }
                }
                if !oversized && let Some(open) = model.cache_mut().open_threads.get_mut(&thread) {
                    if let Some(window) = &mut open.history {
                        window
                            .transcript
                            .apply_loaded_entry(entry, (*loaded).clone());
                    }
                    open.transcript.apply_loaded_entry(entry, *loaded);
                }
            }
        }
        Err(error) => {
            if let Some(EntryAction::Preview(request)) = action
                && let Some(preview) = model
                    .timeline
                    .file_preview
                    .as_mut()
                    .filter(|p| p.request == request)
            {
                preview.body = FilePreviewBody::Error(error);
            } else {
                effects.push(Effect::Toast {
                    title: "Couldn’t load full entry".into(),
                    detail: Some(error),
                });
            }
        }
    }
    if let Some((next_thread, next_key, next_entry)) = next
        && model.connected()
    {
        effects.push(Effect::Server(ServerEffect::ReadEntry {
            thread: next_thread,
            key: next_key,
            entry: next_entry,
        }));
    }
}

fn copy_feedback(model: &mut Model, key: String, text: String, effects: &mut Vec<Effect>) {
    model.timeline.copied_key = Some(key);
    effects.push(Effect::CopyText(text));
}

fn request_entry(model: &mut Model, key: String, entry: EntryId, effects: &mut Vec<Effect>) {
    if model.connected()
        && let Some(thread) = model.selection.selected
        && model
            .connection
            .live
            .request_entry(thread, key.clone(), entry)
    {
        effects.push(Effect::Server(ServerEffect::ReadEntry {
            thread,
            key,
            entry,
        }));
    }
}

fn fork_before(thread: ThreadId, entry: EntryId, load_prompt: bool, effects: &mut Vec<Effect>) {
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

fn open_entry_preview(
    model: &mut Model,
    key: String,
    entry: EntryId,
    title: String,
    effects: &mut Vec<Effect>,
) {
    let Some(thread) = model.selection.selected.filter(|_| model.connected()) else {
        return;
    };
    model.timeline.file_preview_seq = model.timeline.file_preview_seq.wrapping_add(1);
    let request = model.timeline.file_preview_seq;
    model.timeline.file_preview = Some(FilePreview {
        request,
        path: title,
        lines: None,
        source: None,
        body: FilePreviewBody::Loading,
    });
    model
        .timeline
        .entry_actions
        .insert((thread, entry), EntryAction::Preview(request));
    effects.push(Effect::ClearPreviewImage);
    request_entry(model, key, entry, effects);
}

fn entry_preview_text(entry: &EntryData) -> Option<String> {
    let tool = match entry {
        EntryData::Prompt {
            prompt: viz_client::protocol::Prompt { text, .. },
            ..
        }
        | EntryData::TaskResult { text, .. } => {
            return Some(text.clone());
        }
        EntryData::Transcript(entry) => match &entry.body {
            viz_client::protocol::Body::Tool(tool) => tool,
            viz_client::protocol::Body::Plan(plan) => {
                return Some(match &plan.content {
                    viz_client::protocol::PlanContent::Markdown { text, .. } => text.clone(),
                    viz_client::protocol::PlanContent::Items { entries } => entries
                        .iter()
                        .map(|entry| entry.content.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
            viz_client::protocol::Body::Compaction(notice) => return Some(notice.summary.clone()),
            _ => return None,
        },
        EntryData::ToolUpdate { record, .. } => record,
        _ => return None,
    };
    tool.full_output_text().or_else(|| {
        let mut output = String::new();
        for diff in tool.diffs() {
            use std::fmt::Write as _;
            let _ = writeln!(output, "{}", diff.path);
            output.push_str(&diff.new_text);
            output.push('\n');
        }
        (!output.is_empty()).then_some(output)
    })
}

const FILE_PREVIEW_WINDOW: u32 = 192;

fn open_file_preview(
    model: &mut Model,
    path: String,
    lines: Option<(usize, usize)>,
    effects: &mut Vec<Effect>,
) {
    model.timeline.file_preview_seq = model.timeline.file_preview_seq.wrapping_add(1);
    let request = model.timeline.file_preview_seq;
    effects.push(Effect::ClearPreviewImage);
    let mut preview = FilePreview {
        request,
        path,
        lines,
        source: None,
        body: FilePreviewBody::Loading,
    };
    match resolve_preview_file(model, &preview.path) {
        Ok((path, source)) => {
            preview.path = path;
            preview.source = Some(source);
            let start = lines.map_or(0, |(start, _)| start.saturating_sub(13));
            read_preview_window(&preview, start, effects);
        }
        Err(error) => preview.body = FilePreviewBody::Error(error.into()),
    }
    model.timeline.file_preview = Some(preview);
}

fn resolve_preview_file(model: &Model, path: &str) -> Result<(String, PreviewFile), &'static str> {
    if !model.connected() {
        return Err("Server is offline.");
    }
    let (root, base) = if let Some(thread) = model.selection.selected {
        (
            FileRoot::Thread { thread },
            model.thread_state().map(|state| state.cwd.as_str()),
        )
    } else {
        let project = model
            .project_path()
            .ok_or("Select a thread or project to preview files.")?;
        (
            FileRoot::Project {
                project: project.to_owned(),
            },
            Some(project),
        )
    };
    let relative = workspace_relative_path(base, path)
        .and_then(|path| viz_fs::RelPath::parse(&path).ok())
        .ok_or("File is outside the active workspace.")?;
    let local_root = base
        .filter(|_| model.connection.live.local_host)
        .map(std::path::PathBuf::from);
    Ok((
        relative.to_string(),
        PreviewFile {
            root,
            session: model.connection.live.id,
            local_root,
        },
    ))
}

fn active_preview_file(model: &Model, request: u64) -> Option<&FilePreview> {
    model.timeline.file_preview.as_ref().filter(|preview| {
        model.connected()
            && preview.request == request
            && preview
                .source
                .as_ref()
                .is_some_and(|file| file.session == model.connection.live.id)
    })
}

pub(super) fn read_preview_window(preview: &FilePreview, start: usize, effects: &mut Vec<Effect>) {
    if let Some(file) = &preview.source {
        effects.push(Effect::Server(ServerEffect::ReadFile {
            request: preview.request,
            root: file.root.clone(),
            path: preview.path.clone(),
            window: LineWindow {
                start: u32::try_from(start.saturating_add(1)).unwrap_or(u32::MAX),
                count: FILE_PREVIEW_WINDOW,
            },
        }));
    }
}

pub(super) fn file_preview_visible(
    model: &mut Model,
    request: u64,
    rows: std::ops::Range<usize>,
    effects: &mut Vec<Effect>,
) {
    if rows.is_empty() || active_preview_file(model, request).is_none() {
        return;
    }
    let Some(preview) = model.timeline.file_preview.as_mut() else {
        return;
    };
    if let FilePreviewBody::Text { lines, .. } = &mut preview.body {
        if lines.pending {
            lines.visible_while_pending = Some(rows);
        } else if let Some(start) = lines.next_start(&rows) {
            lines.pending = true;
            read_preview_window(preview, start, effects);
        }
    }
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
        source: None,
        body: if mime.starts_with("image/") {
            crate::model::FilePreviewBody::Image { size: 0 }
        } else {
            crate::model::FilePreviewBody::Loading
        },
    });
    if !model.connected() {
        if let Some(preview) = model.timeline.file_preview.as_mut() {
            preview.body = crate::model::FilePreviewBody::Error("Server is offline.".into());
        }
        return;
    }
    effects.push(Effect::Server(ServerEffect::ReadAttachment {
        thread,
        id,
        preview_request: Some(request),
    }));
}

pub(super) fn apply_loaded_attachment_to_preview(
    model: &mut Model,
    request: u64,
    mime: String,
    bytes: Vec<u8>,
    effects: &mut Vec<Effect>,
) {
    let Some(preview) = model.timeline.file_preview.as_mut() else {
        return;
    };
    if preview.request != request {
        return;
    }
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if mime.starts_with("image/") {
        preview.body = crate::model::FilePreviewBody::Image { size };
        effects.push(Effect::DecodePreviewImage {
            media_type: mime,
            bytes,
        });
    } else if let Ok(text) = String::from_utf8(bytes) {
        preview.body = crate::model::FilePreviewBody::Text {
            lines: PreviewLines::new(&preview.path, &text, 0, text.split_terminator('\n').count()),
            size,
        };
    } else {
        preview.body = crate::model::FilePreviewBody::Binary { mime, size };
    }
}

fn lookup_attachment_meta(model: &Model, thread: ThreadId, id: &str) -> (String, String) {
    if let Some(open) = model.cache().open_threads.get(&thread) {
        for item in &open.transcript.items {
            if let Item::User(user) = item
                && let Some(att) = user.prompt.attachments().find(|a| a.id == id)
            {
                return (att.name, att.mime);
            }
        }
    }
    (id.to_owned(), "application/octet-stream".into())
}
