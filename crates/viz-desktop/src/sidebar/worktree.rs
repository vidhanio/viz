use rune::{El, TextEdit, Tx, col, row, text};
use rune_kit::{ButtonVariant, Dialog};
use viz_client::protocol::{Project, Worktree};

use crate::{app::Desktop, theme::Theme, ui};

const BRANCH_FIELD: &str = "worktree-branch";

#[derive(Debug, Default)]
pub(super) struct Form {
    project: Option<String>,
    branch: TextEdit,
    base: TextEdit,
}

impl Form {
    pub(super) fn for_project(project: &Project) -> Option<Self> {
        let base = project.branch.as_deref()?;
        let mut slug = String::with_capacity(project.name.len() + "-worktree".len());
        for word in project.name.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if word.is_empty() {
                continue;
            }
            slug.extend(word.chars().map(|ch| ch.to_ascii_lowercase()));
            slug.push('-');
        }
        slug.push_str("worktree");
        let len = slug.len();
        let mut branch = TextEdit::new(slug);
        branch.set_selection(0, len);
        Some(Self {
            project: Some(project.id.clone()),
            branch,
            base: TextEdit::new(base),
        })
    }
}

pub(super) fn open(s: &mut Desktop, project: &str, tx: &mut Tx) {
    if !s.connected() || s.new_chat.is_some() {
        return;
    }
    if let Some(form) = s
        .projects
        .iter()
        .find(|candidate| candidate.id == project)
        .and_then(Form::for_project)
    {
        s.navigation.menu = None;
        s.navigation.worktree = form;
        tx.focus_when_shown(BRANCH_FIELD, false);
    }
}

fn close(s: &mut Desktop) {
    s.navigation.worktree.project = None;
}

fn submit(s: &mut Desktop, tx: &mut Tx) {
    if !s.connected() || s.new_chat.is_some() {
        return;
    }
    let form = &mut s.navigation.worktree;
    let branch = form.branch.text.trim();
    let base = form.base.text.trim();
    if !branch.is_empty()
        && let Some(project) = form.project.take()
    {
        let worktree = Worktree {
            branch: branch.into(),
            base: (!base.is_empty()).then(|| base.into()),
        };
        s.create_worktree(project, worktree, tx);
    }
}

fn field(
    value: &TextEdit,
    key: &str,
    label: &str,
    placeholder: &str,
    edit: impl Fn(&mut Desktop) -> &mut TextEdit + 'static,
    t: &Theme,
) -> El {
    col()
        .w_full()
        .gap(5.0)
        .child(text(label).font(t.small).color(t.muted))
        .child(
            ui::text_field(value, label, placeholder)
                .on_edit(edit)
                .on_submit(submit)
                .build(&t.k)
                .key(key),
        )
}

pub(super) fn dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let form = &s.navigation.worktree;
    let Some(id) = &form.project else {
        return;
    };
    let mut dialog = Dialog::new("new-worktree", "New worktree chat")
        .open(true)
        .fit()
        .width(408.0)
        .on_close(close);
    if let Some(project) = s.projects.iter().find(|project| project.id == *id) {
        dialog = dialog.description(format!(
            "A new branch in its own checkout of {}.",
            project.name
        ));
    }
    dialog
        .child(
            col()
                .w_full()
                .px(16.0)
                .pb(16.0)
                .gap(12.0)
                .child(field(
                    &form.branch,
                    BRANCH_FIELD,
                    "Branch",
                    "New branch name",
                    |s| &mut s.navigation.worktree.branch,
                    t,
                ))
                .child(field(
                    &form.base,
                    "worktree-base",
                    "Base",
                    "Project’s current branch",
                    |s| &mut s.navigation.worktree.base,
                    t,
                ))
                .child(
                    row()
                        .w_full()
                        .justify_end()
                        .gap(6.0)
                        .child(
                            ui::button("Cancel", ButtonVariant::Ghost)
                                .on_press(close)
                                .build(&t.k),
                        )
                        .child(
                            ui::button("Create chat", ButtonVariant::Submit)
                                .disabled(
                                    !s.connected()
                                        || s.new_chat.is_some()
                                        || form.branch.text.trim().is_empty(),
                                )
                                .on_press_tx(submit)
                                .build(&t.k),
                        ),
                ),
        )
        .build(&t.k, layer);
}
