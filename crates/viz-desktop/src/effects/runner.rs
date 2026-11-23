//! Imperative execution of [`Effect`] values against [`Desktop`] and Rune's
//! [`Tx`].

use rune::{FileDialogOptions, TextEdit, Tx};

use super::{EditSync, EditTarget, Effect, ServerEffect, prefs};
use crate::{
    app::Desktop,
    update::{self, Msg, ServerMsg, SidebarMsg},
};

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
                Effect::Notify { title, body } => {
                    self.send_server(ServerEffect::Notify { title, body }, tx.as_deref_mut());
                }
                Effect::SavePrefs(data) => {
                    if let Some(path) = self.prefs_path() {
                        prefs::save(path, &data);
                    }
                }
                Effect::PickProjectFolder => {
                    if let Some(tx) = tx.as_deref_mut() {
                        let options = FileDialogOptions {
                            title: "Add project".into(),
                            ..FileDialogOptions::default()
                        };
                        tx.open_folder(options, |s: &mut Self, result, tx| match result {
                            Ok(Some(path)) => s.dispatch(
                                Msg::Sidebar(SidebarMsg::ProjectFolderPicked(
                                    path.to_string_lossy().into_owned(),
                                )),
                                tx,
                            ),
                            Ok(None) => {}
                            Err(error) => s.dispatch(
                                Msg::Sidebar(SidebarMsg::ProjectFolderFailed(error.to_string())),
                                tx,
                            ),
                        });
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
                Effect::ScrollKeyCenterAt { key, center_y } => {
                    if let Some(tx) = tx.as_deref_mut() {
                        tx.scroll_key_center_at(key, center_y);
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

    fn send_server(&mut self, command: ServerEffect, mut tx: Option<&mut Tx>) {
        if let Some(error) = self.send_command(command) {
            let recovery = match error {
                ServerEffect::Answer(answer) => {
                    let effects = update::update(
                        &mut self.model,
                        Msg::Requests(crate::update::requests::RequestMsg::Finished(
                            answer.request,
                            Err("Server disconnected".into()),
                        )),
                    );
                    self.apply_effects(effects, tx);
                    return;
                }
                ServerEffect::Create { draft, params } => Some(ServerMsg::LaunchFailed {
                    draft,
                    params: Box::new(params),
                    title: "Offline".into(),
                    detail: None,
                }),
                ServerEffect::SendInput(input) => Some(ServerMsg::SendFailed {
                    input: Box::new(input),
                    title: "Offline".into(),
                    detail: None,
                }),
                ServerEffect::Update { op_id, .. }
                | ServerEffect::Project {
                    op_id: Some(op_id), ..
                } => Some(ServerMsg::MutationFailed {
                    op_id,
                    title: "Offline".into(),
                    detail: "Server disconnected".into(),
                }),
                ServerEffect::LoadDraft(draft) => Some(ServerMsg::DraftHarnessesLoaded {
                    draft,
                    result: None,
                }),
                ServerEffect::SearchThreads { request, .. } => {
                    Some(ServerMsg::ThreadSearchResult {
                        request,
                        result: Err("The server is offline.".into()),
                    })
                }
                _ => {
                    if let Some(tx) = tx.as_deref_mut() {
                        self.show_toast(tx, "Offline", None);
                    }
                    None
                }
            };
            if let Some(msg) = recovery {
                let follow_up = update::update(&mut self.model, Msg::Server(msg));
                self.apply_effects(follow_up, tx);
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
            EditSync::Clear(target) => {
                self.edit_mut(target).clear();
            }
            EditSync::PrependDraft(text) => {
                let draft = &mut self.widgets.draft;
                if !text.is_empty() {
                    if !draft.text.is_empty() {
                        draft.set_caret(0);
                        draft.insert("\n\n");
                        draft.set_caret(0);
                    }
                    draft.insert(&text);
                }
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
                self.model.composer.draft.text.clone_from(&draft.text);
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
        }
    }
}
