//! Modal Git actions. The backend owns every repository operation.

use rune::{El, ScrollSpec, TextEdit, Tx, col, icon, row, text};
use rune_kit::{ButtonVariant, Checkbox, TextArea};
use viz_client::protocol::{Branches, CommitResult, EntryId};

use super::{Request, target};
use crate::{
    app::Desktop,
    backend::Command,
    theme::{DIALOG_INSET, HAIRLINE, ICON, S8, Size, TREE_ROW, Theme},
    ui,
};

/// The branch list shows this many rows before it scrolls.
const BRANCH_ROWS: f32 = 7.0;

const BRANCH_SEARCH: &str = "git-branch-search";
const BRANCH_NAME: &str = "git-branch-name";
const COMMIT_MESSAGE: &str = "git-commit-message";
const PR_TITLE: &str = "git-pr-title";
const PR_OPEN: &str = "git-pr-open";

#[derive(Debug)]
enum Modal {
    Branches,
    NewBranch,
    Commit,
    /// The commit request is in flight; the form's message is kept for a retry.
    Committing,
    Committed(CommitResult),
    Pr,
    Revert(EntryId),
    Created(String),
}

#[derive(Debug, Default)]
enum BranchLoad {
    #[default]
    Idle,
    Pending,
    Ready(Branches),
    Failed(String),
}

#[derive(Debug, Default)]
pub(super) struct State {
    modal: Option<Modal>,
    branches: BranchLoad,
    filter: TextEdit,
    value: TextEdit,
    body: TextEdit,
    draft: bool,
}

impl State {
    pub(super) fn set_branches(&mut self, result: Result<Branches, String>) {
        if matches!(self.branches, BranchLoad::Pending) {
            self.branches = match result {
                Ok(branches) => BranchLoad::Ready(branches),
                Err(error) => BranchLoad::Failed(error),
            };
        }
    }

    pub(super) fn preview_commit(&mut self) {
        self.value.clear();
        self.modal = Some(Modal::Commit);
    }
}

/// Open the repository's local branch picker.
pub fn branch_dialog(s: &mut Desktop, tx: &mut Tx) {
    if target(s).is_some() {
        s.changes.dialog.filter.clear();
        back_to_branches(s, tx);
        load_branches(s, tx);
    }
}

fn load_branches(s: &mut Desktop, tx: &mut Tx) {
    let Some(conversation) = target(s) else {
        return;
    };
    if matches!(s.changes.dialog.branches, BranchLoad::Pending) {
        return;
    }
    s.changes.dialog.branches = BranchLoad::Pending;
    if !s.send(Command::Git(Request::Branches(conversation)), tx) {
        s.changes
            .set_branches(Err("The backend is offline.".into()));
    }
}

fn back_to_branches(s: &mut Desktop, tx: &mut Tx) {
    s.changes.dialog.modal = Some(Modal::Branches);
    tx.focus_when_shown(BRANCH_SEARCH, false);
}

fn new_branch(s: &mut Desktop, tx: &mut Tx) {
    s.changes.dialog.value.clear();
    s.changes.dialog.modal = Some(Modal::NewBranch);
    tx.focus_when_shown(BRANCH_NAME, false);
}

/// Ask for an optional message before staging and committing all changes.
pub fn commit_dialog(s: &mut Desktop, tx: &mut Tx) {
    if target(s).is_some() {
        s.changes.dialog.value.clear();
        s.changes.dialog.modal = Some(Modal::Commit);
        tx.focus_when_shown(COMMIT_MESSAGE, false);
    }
}

/// Ask for pull request details. Blank fields let the backend fill them.
pub fn pr_dialog(s: &mut Desktop, tx: &mut Tx) {
    if target(s).is_some() {
        let dialog = &mut s.changes.dialog;
        dialog.value.clear();
        dialog.body.clear();
        dialog.draft = false;
        dialog.modal = Some(Modal::Pr);
        tx.focus_when_shown(PR_TITLE, false);
    }
}

/// Ask before restoring the working tree to a turn's checkpoint.
pub fn confirm_revert(s: &mut Desktop, tx: &mut Tx, turn: EntryId) {
    if target(s).is_some() {
        s.changes.dialog.modal = Some(Modal::Revert(turn));
        tx.focus_when_shown("git-revert-cancel", false);
    }
}

/// Show the new commit in the dialog that asked for it.
pub fn commit_result(s: &mut Desktop, result: Result<CommitResult, String>, tx: &mut Tx) {
    if !matches!(s.changes.dialog.modal, Some(Modal::Committing)) {
        if let Ok(result) = result {
            s.toast(tx, "Committed", Some(result.message));
        }
        return;
    }
    match result {
        Ok(result) => {
            s.changes.dialog.modal = Some(Modal::Committed(result));
            tx.focus_when_shown("git-commit-done", false);
        }
        Err(error) => {
            s.changes.dialog.modal = Some(Modal::Commit);
            s.toast(tx, "Commit failed", Some(error));
        }
    }
}

/// Display the created pull request's link and offer to open it in the browser.
pub fn pr_result(s: &mut Desktop, url: String, tx: &mut Tx) {
    if s.changes.dialog.modal.is_some() {
        // Don't replace an unrelated form the user opened while the request was
        // in flight.
        let error = tx.open_url(&url).err();
        if let Some(error) = error {
            s.toast(tx, "Couldn’t open pull request", Some(error.to_string()));
        } else {
            s.toast(tx, "Pull request created", Some(url));
        }
    } else {
        s.changes.dialog.modal = Some(Modal::Created(url));
        tx.focus_when_shown(PR_OPEN, false);
    }
}

fn close(s: &mut Desktop) {
    s.changes.dialog.modal = None;
}

fn optional(value: &TextEdit) -> Option<String> {
    let value = value.text.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn submit(s: &mut Desktop, tx: &mut Tx) {
    let Some(conversation) = target(s) else {
        return;
    };
    let dialog = &s.changes.dialog;
    let request = match dialog.modal {
        Some(Modal::NewBranch) => {
            let Some(branch) = optional(&dialog.value) else {
                return;
            };
            Request::Checkout {
                conversation,
                branch,
                create: true,
            }
        }
        Some(Modal::Commit) => Request::Commit {
            conversation,
            message: optional(&dialog.value),
        },
        Some(Modal::Pr) => Request::Pr {
            conversation,
            title: optional(&dialog.value),
            body: optional(&dialog.body),
            draft: dialog.draft,
        },
        Some(Modal::Revert(turn)) => Request::Revert { conversation, turn },
        _ => return,
    };
    let committing = matches!(request, Request::Commit { .. });
    if s.send(Command::Git(request), tx) {
        if committing {
            s.changes.dialog.modal = Some(Modal::Committing);
        } else {
            close(s);
        }
    }
}

fn checkout(s: &mut Desktop, index: usize, tx: &mut Tx) {
    let Some(conversation) = target(s) else {
        return;
    };
    let BranchLoad::Ready(branches) = &s.changes.dialog.branches else {
        return;
    };
    let Some(branch) = branches.branches.get(index) else {
        return;
    };
    if branches.current.as_ref() == Some(branch) {
        close(s);
        return;
    }
    let command = Command::Git(Request::Checkout {
        conversation,
        branch: branch.clone(),
        create: false,
    });
    if s.send(command, tx) {
        close(s);
    }
}

fn checkout_from_search(s: &mut Desktop, tx: &mut Tx) {
    let dialog = &s.changes.dialog;
    let BranchLoad::Ready(branches) = &dialog.branches else {
        return;
    };
    let query = dialog.filter.text.trim();
    if query.is_empty() {
        return;
    }
    let mut matches = branches
        .branches
        .iter()
        .enumerate()
        .filter(|(_, branch)| branch.contains(query));
    if let Some(index) = branches.branches.iter().position(|branch| branch == query) {
        checkout(s, index, tx);
    } else if let Some((index, _)) = matches.next()
        && matches.next().is_none()
    {
        checkout(s, index, tx);
    }
}

fn open_pr(s: &mut Desktop, tx: &mut Tx) {
    let Some(Modal::Created(url)) = &s.changes.dialog.modal else {
        return;
    };
    match tx.open_url(url) {
        Ok(()) => close(s),
        Err(error) => s.toast(tx, "Couldn’t open pull request", Some(error.to_string())),
    }
}

/// Mount the active Git action once at the root layer, even when the inspector
/// is closed.
pub fn overlays(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let Some(modal) = &s.changes.dialog.modal else {
        return;
    };
    let disabled = target(s).is_none();
    let (title, detail, body) = match modal {
        Modal::Branches => (
            "Switch branch",
            "Choose a local branch for this working tree.",
            branch_picker(s, disabled, t),
        ),
        Modal::NewBranch => (
            "Create branch",
            "Create a branch at the current commit and switch to it.",
            new_branch_form(&s.changes.dialog, disabled, t),
        ),
        Modal::Commit => (
            "Commit changes",
            "Stage and commit all working tree changes.",
            commit_form(&s.changes.dialog, disabled, t),
        ),
        Modal::Committing => ("Commit changes", "", busy("Committing", t)),
        Modal::Committed(result) => ("Committed", "", commit_summary(result, t)),
        Modal::Pr => (
            "Create pull request",
            "Leave fields blank to use the branch’s commit messages.",
            pr_form(&s.changes.dialog, disabled, t),
        ),
        Modal::Revert(turn) => ("Revert turn", "", revert_form(*turn, disabled, t)),
        Modal::Created(url) => ("Pull request created", "", pr_link(url, t)),
    };
    ui::dialog("git-action", title, (!detail.is_empty()).then_some(detail)).build(
        close,
        col().w_full().px(DIALOG_INSET).pb(DIALOG_INSET).child(body),
        t,
        layer,
    );
}

/// A spinner and what it's waiting for, one control tall.
fn busy(label: &str, t: &Theme) -> El {
    row()
        .h(Size::Default.height())
        .gap(S8)
        .center_y()
        .child(ui::spinner(t))
        .child(text(label).font(t.ui).color(t.muted))
}

fn branch_picker(s: &Desktop, disabled: bool, t: &Theme) -> El {
    let dialog = &s.changes.dialog;
    let body = match &dialog.branches {
        BranchLoad::Idle | BranchLoad::Pending => busy("Loading branches", t),
        BranchLoad::Failed(error) => col()
            .gap(S8)
            .child(text(error).font(t.ui).color(t.danger).wrap_anywhere())
            .child(
                ui::button("Retry", ButtonVariant::Ghost, Size::Default)
                    .disabled(disabled)
                    .on_press_tx(load_branches)
                    .build(&t.k),
            ),
        BranchLoad::Ready(branches) => {
            branch_list(branches, dialog.filter.text.trim(), disabled, t)
        }
    };
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(
            ui::text_field(
                &dialog.filter,
                "Filter branches",
                "Filter branches",
                Size::Default,
            )
            .leading_icon(&ui::SEARCH)
            .on_edit(|s: &mut Desktop| &mut s.changes.dialog.filter)
            .on_submit(checkout_from_search)
            .build(&t.k)
            .key(BRANCH_SEARCH),
        )
        .child(body)
        .child(ui::dialog_actions([
            cancel(t),
            ui::button("New branch…", ButtonVariant::Submit, Size::Default)
                .disabled(disabled)
                .on_press_tx(new_branch)
                .build(&t.k),
        ]))
}

fn branch_list(branches: &Branches, query: &str, disabled: bool, t: &Theme) -> El {
    let mut rows = branches
        .branches
        .iter()
        .enumerate()
        .filter(|(_, branch)| branch.contains(query))
        .peekable();
    if rows.peek().is_none() {
        let message = if query.is_empty() {
            "No local branches"
        } else {
            "No matching branches"
        };
        return text(message)
            .font(t.ui)
            .color(t.muted)
            .h(Size::Default.height());
    }
    col()
        .role("listbox")
        .label("Local branches")
        .focus_group()
        .w_full()
        .max_h(BRANCH_ROWS * TREE_ROW)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .children(rows.map(|(i, name)| {
            let current = branches.current.as_ref() == Some(name);
            let mut branch = row()
                .key(format!("git-branch-{name}"))
                .role("option")
                .label(name)
                .focusable(!disabled)
                .focus_selected(current)
                .focus_ring(HAIRLINE, -HAIRLINE, t.accent)
                .disabled(disabled)
                .w_full()
                .h(TREE_ROW)
                .px(S8)
                .gap(S8)
                .center_y()
                .radius(Size::Default.radius())
                .child(
                    text(name)
                        .font(t.ui)
                        .color(t.text)
                        .grow(1.0)
                        .shrink()
                        .truncate()
                        .a11y_hidden(),
                );
            if current {
                branch = branch
                    .description("Current branch")
                    .child(icon(&ui::CHECK, ICON).color(t.accent).a11y_hidden());
            }
            if !disabled {
                branch = branch
                    .hover(t.hover)
                    .on_press_tx(move |s, tx| checkout(s, i, tx));
            }
            branch
        }))
}

fn new_branch_form(state: &State, disabled: bool, t: &Theme) -> El {
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(ui::field(
            "Branch name",
            ui::text_field(
                &state.value,
                "Branch name",
                "feature/project-routes",
                Size::Default,
            )
            .on_edit(|s: &mut Desktop| &mut s.changes.dialog.value)
            .on_submit(submit)
            .build(&t.k)
            .key(BRANCH_NAME),
            t,
        ))
        .child(ui::dialog_actions([
            ui::button("Back", ButtonVariant::Ghost, Size::Default)
                .on_press_tx(back_to_branches)
                .build(&t.k),
            action(
                "Create branch",
                ButtonVariant::Submit,
                disabled || state.value.text.trim().is_empty(),
                t,
            ),
        ]))
}

fn commit_form(state: &State, disabled: bool, t: &Theme) -> El {
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(ui::field(
            "Message · optional",
            TextArea::new(&state.value)
                .width(None)
                .label("Commit message")
                .placeholder("Leave blank to generate one")
                .auto_grow(3, 6)
                .on_edit(|s: &mut Desktop| &mut s.changes.dialog.value)
                .on_submit(submit)
                .build(&t.k)
                .key(COMMIT_MESSAGE),
            t,
        ))
        .child(ui::dialog_actions([
            cancel(t),
            action("Commit", ButtonVariant::Submit, disabled, t),
        ]))
}

fn pr_form(state: &State, disabled: bool, t: &Theme) -> El {
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(ui::field(
            "Title · optional",
            ui::text_field(
                &state.value,
                "Pull request title",
                "Use the commit subject",
                Size::Default,
            )
            .on_edit(|s: &mut Desktop| &mut s.changes.dialog.value)
            .build(&t.k)
            .key(PR_TITLE),
            t,
        ))
        .child(ui::field(
            "Description · optional",
            TextArea::new(&state.body)
                .width(None)
                .label("Pull request description")
                .placeholder("Use the commit messages")
                .auto_grow(3, 6)
                .on_edit(|s: &mut Desktop| &mut s.changes.dialog.body)
                .on_submit(submit)
                .build(&t.k),
            t,
        ))
        .child(
            Checkbox::new(state.draft)
                .label("Draft")
                .disabled(disabled)
                .on_toggle(|s: &mut Desktop| s.changes.dialog.draft = !s.changes.dialog.draft)
                .build(&t.k),
        )
        .child(ui::dialog_actions([
            cancel(t),
            action("Create pull request", ButtonVariant::Submit, disabled, t),
        ]))
}

fn revert_form(turn: EntryId, disabled: bool, t: &Theme) -> El {
    let detail = format!(
        "Restore the working tree to the start of turn #{}? This discards every file change since, manual edits included.",
        turn.0
    );
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(text(detail).font(t.ui).color(t.muted).wrap())
        .child(ui::dialog_actions([
            cancel(t).key("git-revert-cancel"),
            action("Revert turn", ButtonVariant::Danger, disabled, t),
        ]))
}

fn commit_summary(result: &CommitResult, t: &Theme) -> El {
    let hash = result.commit.get(..8).unwrap_or(&result.commit);
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(
            row()
                .w_full()
                .gap(S8)
                .items_start()
                .child(text(hash).font(t.mono).color(t.muted).selectable(true))
                .child(
                    text(&result.message)
                        .font(t.ui)
                        .color(t.text)
                        .wrap_anywhere()
                        .grow(1.0)
                        .shrink()
                        .selectable(true),
                ),
        )
        .child(ui::dialog_actions([
            ui::button("Done", ButtonVariant::Ghost, Size::Default)
                .on_press(close)
                .build(&t.k)
                .key("git-commit-done"),
            ui::button("Push", ButtonVariant::Submit, Size::Default)
                .on_press_tx(|s: &mut Desktop, tx| {
                    close(s);
                    super::push(s, tx);
                })
                .build(&t.k),
        ]))
}

fn pr_link(url: &str, t: &Theme) -> El {
    col()
        .w_full()
        .gap(DIALOG_INSET)
        .child(
            text(url)
                .font(t.ui)
                .color(t.accent)
                .wrap_anywhere()
                .selectable(true),
        )
        .child(ui::dialog_actions([
            ui::button("Done", ButtonVariant::Ghost, Size::Default)
                .on_press(close)
                .build(&t.k),
            ui::button("Open in browser", ButtonVariant::Submit, Size::Default)
                .on_press_tx(open_pr)
                .build(&t.k)
                .key(PR_OPEN),
        ]))
}

fn cancel(t: &Theme) -> El {
    ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
        .on_press(close)
        .build(&t.k)
}

fn action(label: &str, variant: ButtonVariant, disabled: bool, t: &Theme) -> El {
    ui::button(label, variant, Size::Default)
        .disabled(disabled)
        .on_press_tx(submit)
        .build(&t.k)
}
