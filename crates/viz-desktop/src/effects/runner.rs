//! Imperative execution of [`Effect`] values against [`Desktop`] and Rune's
//! [`Tx`].

use rune::{TextEdit, Tx};

use super::{EditSync, EditTarget, Effect, ServerEffect, prefs};
use crate::{app::Desktop, update};

impl Desktop {
    /// Executes every effect returned by [`crate::update::update`].
    #[expect(
        clippy::too_many_lines,
        reason = "single exhaustive interpreter over every Effect variant"
    )]
    pub(crate) fn apply_effects(&mut self, effects: Vec<Effect>, mut tx: Option<&mut Tx>) {
        for effect in effects {
            match effect {
                Effect::Server(command) => self.send_server(command, tx.as_deref_mut()),
                Effect::Attach {
                    scope,
                    target,
                    source,
                } => self.send_server(
                    ServerEffect::Attach {
                        scope,
                        target,
                        source,
                    },
                    tx.as_deref_mut(),
                ),
                Effect::Notify {
                    thread,
                    title,
                    body,
                } => {
                    self.send_server(
                        ServerEffect::Notify {
                            thread,
                            title,
                            body,
                        },
                        tx.as_deref_mut(),
                    );
                }
                Effect::SavePrefs(data) => {
                    if let Some(path) = self.prefs_path() {
                        prefs::save(path, &data);
                    }
                }
                Effect::CopyText(text) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.copy(text);
                    }
                }
                Effect::OpenUrl(url) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        _ = tx.open_url(&url);
                    }
                }
                Effect::SetWindowTitle(title) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.set_title(title);
                    }
                }
                Effect::Toast { title, detail } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        self.show_toast(tx, title, detail);
                    }
                }
                Effect::UndoToast {
                    title,
                    detail,
                    undo,
                    secs,
                } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        self.show_undo_toast(tx, title, detail, undo, secs);
                    }
                }
                Effect::DecodeProjectIcon {
                    environment,
                    project,
                    media_type,
                    bytes,
                } => {
                    self.decode_project_icon(&environment, &project, &media_type, bytes);
                }
                Effect::DecodePreviewImage { media_type, bytes } => {
                    self.decode_preview_image(&media_type, bytes);
                }
                Effect::ClearPreviewImage => {
                    self.widgets.preview_image = None;
                }
                Effect::DecodeThreadAttachmentImage {
                    environment,
                    thread,
                    id,
                    media_type,
                    bytes,
                } => {
                    self.decode_thread_attachment_image(
                        &environment,
                        thread,
                        id,
                        &media_type,
                        bytes,
                    );
                }
                Effect::Focus {
                    key,
                    select_all,
                    when_shown,
                } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        if when_shown {
                            tx.focus_when_shown(key, select_all);
                        } else {
                            tx.focus(key, select_all);
                        }
                    }
                }
                Effect::ScrollToBottom(key) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_to_bottom(key);
                    }
                }
                Effect::ScrollToInstant { key, x, y } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_to_instant(key, x, y);
                    }
                }
                Effect::ScrollIntoView { list, item } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_into_view_in(list, item);
                    }
                }
                Effect::ScrollCenterInstant { list, item } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_center_in_instant(list, item);
                    }
                }
                Effect::RevealMessage { thread, item } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_center_in_instant(format!("page-{thread}"), item);
                    }
                }
                Effect::KeepOnScreen(key) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.keep_on_screen(key);
                    }
                }
                Effect::WakeAfter(duration) => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.wake_after(duration);
                    }
                }
                Effect::SyncEdit(sync) => self.apply_edit_sync(sync),
                Effect::SyncSidebarPane { open, width } => {
                    if let Some(width) = width {
                        self.widgets.sidebar_pane.set_width(width);
                    }
                    if let Some(open) = open {
                        self.widgets.sidebar_pane.set_open(open);
                    }
                }
                Effect::SyncDiffPane { open, width } => {
                    if let Some(width) = width {
                        self.widgets.diff_pane.set_width(width);
                    }
                    if let Some(open) = open {
                        self.widgets.diff_pane.set_open(open);
                    }
                }
                Effect::RevealDiffFile(path) => {
                    crate::git::diff_panel::sync(self);
                    if let Some(tx) = tx.as_deref_mut()
                        && let Some(index) = self.widgets.git_diff.file_index(&path)
                    {
                        self.widgets.git_diff.set_collapsed(index, false);
                        self.widgets.git_diff.scroll_to_file(index, tx);
                    }
                }
            }
        }
    }

    fn send_server(&mut self, command: ServerEffect, tx: Option<&mut Tx>) {
        if let Some(failed) = self.send_command(command) {
            let env = self.model.active_env_key().to_owned();
            match super::offline::derive(failed, &env) {
                super::offline::Offline::Dispatch(msg) => {
                    let follow_up = update::update(&mut self.model, msg);
                    self.apply_effects(follow_up, tx);
                }
                super::offline::Offline::Toast => {
                    if let Some(tx) = tx {
                        self.show_toast(tx, "Offline", None);
                    }
                }
                super::offline::Offline::Attach { .. }
                | super::offline::Offline::Notify { .. }
                | super::offline::Offline::Select(_)
                | super::offline::Offline::ClearSelection
                | super::offline::Offline::SwitchEnv { .. }
                | super::offline::Offline::Reconnect
                | super::offline::Offline::Ignore => {}
            }
        }
    }

    fn apply_edit_sync(&mut self, sync: EditSync) {
        match sync {
            EditSync::Replace {
                target,
                text,
                select_all,
            } => {
                let edit = self.edit_mut(target);
                edit.replace_text(&text);
                if select_all {
                    edit.set_selection(0, text.len());
                }
            }
            EditSync::ReplaceDraftWithPills { text, pills } => {
                self.replace_draft_with_pills(&text, &pills);
            }
            EditSync::Clear(target) => {
                self.edit_mut(target).clear();
            }
            EditSync::ReplaceDraftRange {
                start,
                end,
                insertion,
            } => {
                let draft = &mut self.widgets.draft;
                draft.set_selection(start, end);
                draft.insert(&insertion);
                if !insertion.ends_with(' ')
                    && let Some(separator) = draft.text[draft.focus..].chars().next()
                {
                    draft.set_caret(draft.focus + separator.len_utf8());
                }
                self.sync_draft_text();
            }
            EditSync::InsertAtDraftCaret(snippet) => {
                let draft = &mut self.widgets.draft;
                let start = draft.anchor.min(draft.focus).min(draft.text.len());
                let end = draft.anchor.max(draft.focus).min(draft.text.len());
                let need_leading_space = draft.text[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| !ch.is_whitespace());
                let need_trailing_space = draft.text[end..]
                    .chars()
                    .next()
                    .is_none_or(|ch| !ch.is_whitespace());
                let mut insertion = String::with_capacity(snippet.len() + 2);
                if need_leading_space {
                    insertion.push(' ');
                }
                insertion.push_str(&snippet);
                if need_trailing_space {
                    insertion.push(' ');
                }
                draft.insert(&insertion);
                self.sync_draft_text();
            }
            EditSync::InsertDraftPill {
                start,
                end,
                id,
                label,
                trailing_space,
            } => {
                let draft = &mut self.widgets.draft;
                draft.undo_group(|edit| {
                    edit.replace_with_pill(start..end, id, &label);
                    if trailing_space {
                        edit.insert(" ");
                    } else if let Some(separator) = edit.text[edit.focus..].chars().next() {
                        edit.set_caret(edit.focus + separator.len_utf8());
                    }
                });
                self.sync_draft_text();
            }
        }
    }

    const fn edit_mut(&mut self, target: EditTarget) -> &mut TextEdit {
        match target {
            EditTarget::Draft => &mut self.widgets.draft,
            EditTarget::ProjectFilter => &mut self.widgets.project_filter_query,
            EditTarget::InlineRename => &mut self.widgets.inline_rename,
            EditTarget::SidebarDialog => &mut self.widgets.sidebar_dialog,
            EditTarget::PaletteQuery => &mut self.widgets.palette_query,
            EditTarget::ModelQuery => &mut self.widgets.model_query,
            EditTarget::HomeProjectQuery => &mut self.widgets.home_project_query,
            EditTarget::DraftBranchQuery => &mut self.widgets.draft_branch_query,
        }
    }
}
