//! Git quick actions and confirmation forms.

use rune::{El, col, row, text};
use rune_kit::{ButtonVariant, IconButtonVariant, Menu, MenuItem, Placement};
use viz_client::protocol::{GitAction, GitDiffFile, GitStatus};

use crate::{
    app::Desktop,
    effects::git::Request,
    model::git::{self, Dialog, Popup},
    theme::{Size, Theme},
    ui,
    update::GitMsg,
};

const ANCHOR: &str = "git-actions-trigger";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Commit,
    Push,
    Pull,
    Fetch,
    CreatePull,
}

impl Action {
    const fn label(self) -> &'static str {
        match self {
            Self::Commit => "Commit",
            Self::Push => "Push",
            Self::Pull => "Pull",
            Self::Fetch => "Fetch",
            Self::CreatePull => "Create PR",
        }
    }

    const fn glyph(self) -> &'static rune::Icon {
        match self {
            Self::Commit => &ui::GIT_COMMIT,
            Self::Push => &ui::UPLOAD,
            Self::Pull => &ui::DOWNLOAD,
            Self::Fetch => &ui::REFRESH_CW,
            Self::CreatePull => &ui::GIT_FORK,
        }
    }

    fn enabled(self, s: &Desktop, status: &GitStatus) -> bool {
        if !s.model.connected() || s.model.git.busy || s.model.busy() || status.operation.is_some()
        {
            return false;
        }
        match self {
            Self::Commit => status.conflicted == 0 && git::dirty(status),
            Self::Push => status.branch.is_some() && status.commit.is_some() && status.behind == 0,
            Self::Pull => status.behind > 0 && status.ahead == 0 && !git::dirty(status),
            Self::Fetch => true,
            Self::CreatePull => {
                status.branch.is_some()
                    && status.upstream.is_some()
                    && status.commit.is_some()
                    && !needs_push_confirmation(s)
                    && !s
                        .model
                        .selection
                        .selected
                        .and_then(|id| s.model.summary(id))
                        .and_then(|summary| summary.pull_request.as_ref())
                        .is_some_and(|pr| pr.state == viz_client::protocol::GitHubPullState::Open)
            }
        }
    }

    fn activate(self, s: &mut Desktop, tx: &mut rune::Tx) {
        let msg = match self {
            Self::Commit => {
                s.widgets.git_message = rune::TextEdit::new("");
                GitMsg::Dialog(Some(Dialog::Commit))
            }
            Self::Push if needs_push_confirmation(s) => {
                s.widgets.git_message = rune::TextEdit::new("");
                GitMsg::Dialog(Some(Dialog::Push))
            }
            Self::Push => GitMsg::Request(Request::Action(GitAction::Push { remote: remote(s) })),
            Self::Pull => GitMsg::Request(Request::Action(GitAction::Pull)),
            Self::Fetch => GitMsg::Request(Request::Action(GitAction::Fetch { remote: remote(s) })),
            Self::CreatePull => {
                s.widgets.git_message = rune::TextEdit::new("");
                s.widgets.git_body = rune::TextEdit::new("");
                GitMsg::Dialog(Some(Dialog::PullRequest))
            }
        };
        super::dispatch(s, GitMsg::Popup(None), tx);
        super::dispatch(s, msg, tx);
    }
}

fn needs_push_confirmation(s: &Desktop) -> bool {
    let git = &s.model.git;
    git.default_branch.is_none()
        || git
            .status
            .as_ref()
            .and_then(|status| status.branch.as_deref())
            .is_some_and(|branch| {
                branch == git::default_branch(&s.model) || matches!(branch, "main" | "master")
            })
}

fn remote(s: &Desktop) -> String {
    s.model
        .git
        .status
        .as_ref()
        .and_then(|status| status.upstream.as_deref())
        .and_then(|name| name.split_once('/'))
        .map_or_else(|| "origin".into(), |(remote, _)| remote.into())
}

fn primary(s: &Desktop, status: &GitStatus) -> Action {
    if git::dirty(status) {
        Action::Commit
    } else if status.behind > 0 && status.ahead == 0 {
        Action::Pull
    } else if status.ahead > 0 && status.behind == 0 {
        Action::Push
    } else if Action::CreatePull.enabled(s, status) && !needs_push_confirmation(s) {
        Action::CreatePull
    } else {
        Action::Fetch
    }
}

pub fn view(s: &Desktop, status: &GitStatus, t: &Theme, overlays: &mut Vec<El>) -> El {
    let open = s.model.git.popup == Some(Popup::Actions);
    let primary = primary(s, status);
    let busy = s.model.git.busy || status.operation.is_some();
    let action = ui::button(
        if busy {
            "Git action running…"
        } else {
            primary.label()
        },
        ButtonVariant::Ghost,
        Size::Small,
    )
    .icon(primary.glyph())
    .loading(busy)
    .disabled(!primary.enabled(s, status))
    .on_press_tx(move |s: &mut Desktop, tx| primary.activate(s, tx))
    .build(&t.overlay)
    .key("git-primary-action");
    let trigger = ui::icon_button(
        &ui::CHEVRON_OPEN,
        "Git actions",
        IconButtonVariant::Ghost,
        Size::Small,
    )
    .on_press_tx(|s: &mut Desktop, tx| super::toggle_popup(s, Popup::Actions, tx))
    .build(&t.overlay);
    let items = [
        Action::Commit,
        Action::Push,
        Action::Pull,
        Action::Fetch,
        Action::CreatePull,
    ]
    .into_iter()
    .map(|action| {
        MenuItem::new(action.label())
            .icon(action.glyph())
            .disabled(!action.enabled(s, status))
            .on_select_tx(move |s: &mut Desktop, tx| action.activate(s, tx))
    });
    let trigger = ui::menu_items(Menu::new(ANCHOR), items)
        .trigger(trigger)
        .open(open)
        .placement(Placement::BottomEnd)
        .on_close(super::close_popup)
        .build(&t.overlay, overlays);
    row()
        .key("git-action-split")
        .w_full()
        .center_y()
        .hover_group()
        .child(action)
        .child(rune::spacer())
        .child(
            rune::hairline(t.hairline)
                .group_opacity(if open { 1.0 } else { 0.0 }, 1.0)
                .h(16.0),
        )
        .child(trigger)
}

fn paths(file: &GitDiffFile) -> impl Iterator<Item = &str> {
    use viz_client::protocol::GitFileKind;
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .into_iter()
        .chain(
            (file.kind == GitFileKind::Renamed)
                .then_some(file.old_path.as_deref())
                .flatten(),
        )
}

fn commit(s: &mut Desktop, tx: &mut rune::Tx) {
    let Some(diff) = &s.model.git.preview else {
        return;
    };
    let paths = diff
        .files
        .iter()
        .filter(|file| {
            paths(file)
                .next()
                .is_some_and(|path| !s.model.git.excluded.contains(path))
        })
        .flat_map(paths)
        .map(str::to_owned)
        .collect();
    let message = s.widgets.git_message.text.trim().to_owned();
    super::dispatch(
        s,
        GitMsg::Request(Request::Action(GitAction::Commit { paths, message })),
        tx,
    );
}

pub fn push_dialog(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) {
    let git = &s.model.git;
    let branch = git
        .status
        .as_ref()
        .and_then(|status| status.branch.as_deref())
        .unwrap_or("Unknown branch");
    let footer = ui::dialog_actions([
        ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
            .on_press(super::close_dialog)
            .build(&t.k),
        ui::button(
            "Create feature branch",
            ButtonVariant::Outline,
            Size::Default,
        )
        .disabled(git.busy || s.widgets.git_message.text.trim().is_empty())
        .on_press_tx(|s: &mut Desktop, tx| {
            let name = s.widgets.git_message.text.trim().to_owned();
            super::dispatch(
                s,
                GitMsg::Request(Request::CreateCheckout {
                    name,
                    start: "HEAD".into(),
                }),
                tx,
            );
        })
        .build(&t.k),
        ui::button("Push branch", ButtonVariant::Submit, Size::Default)
            .disabled(git.busy)
            .on_press_tx(|s: &mut Desktop, tx| {
                super::dispatch(
                    s,
                    GitMsg::Request(Request::Action(GitAction::Push { remote: remote(s) })),
                    tx,
                );
            })
            .build(&t.k),
    ]);
    let branch_input = rune_kit::Field::new()
        .label("Feature branch name")
        .control(
            rune_kit::TextInput::new(&s.widgets.git_message)
                .label("Feature branch name")
                .placeholder("feature/topic")
                .width(None)
                .on_edit(|s: &mut Desktop| &mut s.widgets.git_message)
                .build(&t.k),
        )
        .build(&t.k);
    rune_kit::Dialog::new("git-push", "Push this branch?")
        .open(true)
        .fit()
        .on_close(super::close_dialog)
        .description("This push can update the repository’s default branch.")
        .child(ui::dialog_subject(branch, t))
        .child(ui::dialog_body(
            std::iter::once(branch_input).chain(error(s, t)),
            footer,
        ))
        .build(&t.overlay, overlays);
}

pub fn commit_dialog(s: &Desktop, t: &Theme, overlays: &mut Vec<El>) {
    let git = &s.model.git;
    let files = git
        .preview
        .as_ref()
        .map_or_default(|diff| diff.files.as_slice());
    let list = col().w_full().children(files.iter().filter_map(|file| {
        let path = paths(file).next()?.to_owned();
        let selected = !git.excluded.contains(&path);
        let stat = file.stats.unwrap_or_default();
        Some(
            ui::list_row(format!("commit-file-{path}"), &path)
                .selected(selected)
                .leading_icon(if selected { &ui::CHECK } else { &ui::FILE_DIFF })
                .trailing(super::diff_panel::stat(stat, t))
                .disabled(git.busy)
                .on_press_tx(move |s: &mut Desktop, tx| {
                    super::dispatch(s, GitMsg::Exclude(path.clone()), tx);
                })
                .build(t),
        )
    }));
    let total = files
        .iter()
        .filter(|file| {
            paths(file)
                .next()
                .is_some_and(|path| !git.excluded.contains(path))
        })
        .count();
    let file_label = if git.preview.is_none() {
        "Loading changed files…".into()
    } else {
        format!("{total} of {} files selected", files.len())
    };
    let fields = [
        Some(
            col()
                .gap(4.0)
                .child(text(file_label).font(t.small).color(t.muted))
                .child(col().h(176.0).child(ui::scroll_viewport(
                    "commit-files",
                    ui::ScrollMode::Top,
                    list,
                    t,
                ))),
        ),
        Some(
            rune_kit::Field::new()
                .label("Commit message")
                .control(
                    rune_kit::TextInput::new(&s.widgets.git_message)
                        .label("Commit message")
                        .placeholder("Describe your changes")
                        .width(None)
                        .on_edit(|s: &mut Desktop| &mut s.widgets.git_message)
                        .build(&t.k),
                )
                .build(&t.k),
        ),
        error(s, t),
    ]
    .into_iter()
    .flatten();
    let footer = ui::dialog_actions([
        ui::button("Cancel", ButtonVariant::Ghost, Size::Default)
            .on_press(super::close_dialog)
            .build(&t.k),
        ui::button(
            if git.busy { "Committing…" } else { "Commit" },
            ButtonVariant::Submit,
            Size::Default,
        )
        .disabled(git.busy || total == 0 || s.widgets.git_message.text.trim().is_empty())
        .on_press_tx(commit)
        .build(&t.k),
    ]);
    rune_kit::Dialog::new("git-commit", "Commit changes")
        .open(true)
        .fit()
        .width(512.0)
        .on_close(super::close_dialog)
        .description("Selected files will be included in the commit.")
        .child(ui::dialog_body(fields, footer))
        .build(&t.overlay, overlays);
}

pub(super) fn error(s: &Desktop, t: &Theme) -> Option<El> {
    s.model.git.error.as_ref().map(|error| {
        text(error)
            .font(t.small)
            .color(t.danger)
            .w_full()
            .wrap_anywhere()
    })
}
