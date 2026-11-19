use rune::{El, TextEdit, Tx};
use rune_kit::ButtonVariant;
use viz_client::protocol::{Project, Worktree};

use crate::{
    app::Desktop,
    theme::{Size, Theme},
    ui,
};

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
    ui::field(
        label,
        ui::text_field(value, label, placeholder, Size::Default)
            .on_edit(edit)
            .on_submit(submit)
            .build(&t.k)
            .key(key),
        t,
    )
}

pub(super) fn dialog(s: &Desktop, t: &Theme, layer: &mut Vec<El>) {
    let form = &s.navigation.worktree;
    let Some(id) = &form.project else {
        return;
    };
    let detail = s
        .projects
        .iter()
        .find(|project| project.id == *id)
        .map(|project| format!("A new branch in its own checkout of {}.", project.name));
    let fields = [
        field(
            &form.branch,
            BRANCH_FIELD,
            "Branch",
            "New branch name",
            |s| &mut s.navigation.worktree.branch,
            t,
        ),
        field(
            &form.base,
            "worktree-base",
            "Base",
            "Project’s current branch",
            |s| &mut s.navigation.worktree.base,
            t,
        ),
    ];
    let actions = ui::dialog_actions([
        ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
            .on_press(close)
            .build(&t.k),
        ui::button("Create chat", ButtonVariant::Submit, Size::Default)
            .disabled(!s.connected() || s.new_chat.is_some() || form.branch.text.trim().is_empty())
            .on_press_tx(submit)
            .build(&t.k),
    ]);
    ui::dialog("new-worktree", "New worktree chat", detail.as_deref())
        .on_close(close)
        .child(ui::dialog_body(fields, actions))
        .build(&t.k, layer);
}
