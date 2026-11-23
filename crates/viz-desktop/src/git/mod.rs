//! Thread details, checkout controls, and docked source review.

mod actions;
mod branch_picker;
pub mod details_card;
pub mod diff_panel;
mod pr_row;
pub mod toolbar;

use rune::{El, Tx};

use crate::{
    app::Desktop,
    model::git::{Dialog, Popup},
    theme::Theme,
    update::{GitMsg, Msg},
};

pub fn dispatch(s: &mut Desktop, msg: GitMsg, tx: &mut Tx) {
    s.dispatch(Msg::Git(msg), tx);
}

fn close_popup(s: &mut Desktop) {
    s.dispatch_local(Msg::Git(GitMsg::Popup(None)));
}

fn close_dialog(s: &mut Desktop) {
    if !s.model.git.busy {
        s.dispatch_local(Msg::Git(GitMsg::Dialog(None)));
    }
}

fn toggle_popup(s: &mut Desktop, popup: Popup, tx: &mut Tx) {
    let next = (s.model.git.popup != Some(popup)).then_some(popup);
    dispatch(s, GitMsg::Popup(next), tx);
}

pub fn dialogs(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    match s.model.git.dialog {
        Some(Dialog::PullRequest) => pr_row::dialog(s, t, layer),
        Some(Dialog::Restore) => diff_panel::restore_dialog(s, t, layer),
        Some(Dialog::Commit) => actions::commit_dialog(s, t, layer),
        Some(Dialog::Push) => actions::push_dialog(s, t, layer),
        None => {}
    }
}
