//! Project workflows presented inside the command palette.

mod settings;
use rune::{El, Icon, col, row, text};
pub use settings::mark;
use viz_client::protocol::{CloneState, EnvironmentStatus, FolderIssue, ImportSession};

use crate::{
    app::Desktop,
    model::{
        Model,
        palette::matches_filter,
        projects::{
            Destination, ProjectAction as Action, ProjectPage, ProjectReply, ProjectView as View,
        },
    },
    theme::{S4, S8, S16, Size, Theme},
    ui,
    update::{Msg, PaletteMsg},
};

pub struct OptionRow {
    title: String,
    subtitle: Option<String>,
    glyph: &'static Icon,
    action: Option<Action>,
    badges: Vec<&'static str>,
    color: Option<usize>,
    notice: bool,
}

impl OptionRow {
    fn new(title: impl Into<String>, glyph: &'static Icon, action: Action) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            glyph,
            action: Some(action),
            badges: Vec::new(),
            color: None,
            notice: false,
        }
    }

    const fn color(mut self, color: usize) -> Self {
        self.color = Some(color);
        self
    }

    fn detail(mut self, value: impl Into<String>) -> Self {
        self.subtitle = Some(value.into());
        self
    }

    fn badges(mut self, tags: Vec<&'static str>) -> Self {
        self.badges = tags;
        self
    }

    fn disabled(mut self, disabled: bool) -> Self {
        if disabled {
            self.action = None;
        }
        self
    }

    pub fn action(&self) -> Option<Action> {
        self.action.clone()
    }

    pub fn build(self, index: usize, active: bool, t: &Theme) -> El {
        let mut item = ui::list_row(option_key(index), self.title)
            .leading_icon(self.glyph)
            .focus_ring_for("palette-query")
            .highlighted(active)
            .disabled(self.action.is_none() && !self.notice);
        if let Some(color) = self.color {
            item = item.leading(ui::project_icon("", Some(self.glyph), 24.0, Some(color), t));
        }
        if let Some(subtitle) = self.subtitle {
            item = item.subtitle(subtitle);
        }
        if !self.badges.is_empty() {
            item = item.trailing(
                row()
                    .gap(S4)
                    .children(self.badges.into_iter().map(|tag| ui::badge(tag).build(t))),
            );
        }
        if let Some(action) = self.action {
            item = item.on_press_tx(move |s: &mut Desktop, tx| {
                s.dispatch(Msg::Palette(PaletteMsg::Project(action.clone())), tx);
            });
        }
        let item = item.build(t).preserve_focus_on_press(true);
        if self.notice {
            item.role("status")
        } else {
            item
        }
    }
}

pub fn option_key(index: usize) -> String {
    format!("project-option-{index}")
}

const fn push(view: View) -> Action {
    Action::Push(view, String::new())
}

pub fn options(model: &Model, page: &ProjectPage) -> Vec<OptionRow> {
    let mut options = match &page.view {
        View::Sources => sources(),
        View::Folders(_) => folder_options(page),
        View::Projects => model
            .cache()
            .projects()
            .map(|project| {
                OptionRow::new(
                    &project.name,
                    &ui::FOLDER,
                    push(View::Settings(project.path.clone())),
                )
                .detail(&project.path)
            })
            .collect(),
        View::Environments => model
            .connection
            .environments
            .iter()
            .map(|env| {
                OptionRow::new(&env.id, &ui::FOLDER_TREE, Action::Switch(env.id.clone()))
                    .detail(match env.status {
                        EnvironmentStatus::Local => "Local",
                        EnvironmentStatus::Reachable => "Connected peer",
                        EnvironmentStatus::Unreachable => "Unavailable",
                    })
                    .disabled(env.status == EnvironmentStatus::Unreachable)
            })
            .collect(),
        View::Harnesses => model
            .harnesses()
            .iter()
            .map(|harness| {
                OptionRow::new(
                    &harness.name,
                    ui::harness_icon(&harness.id),
                    push(View::Sessions {
                        harness: harness.id.clone(),
                        project: model.project().map(|project| project.path.clone()),
                    }),
                )
            })
            .collect(),
        View::Sessions { harness, project } => session_options(page, harness, project.as_deref()),
        View::Repositories => match &page.data {
            Some(ProjectReply::Repositories(repos)) => repos
                .iter()
                .filter_map(|repo| {
                    let action = clone_destination(repo.clone_url.clone())?;
                    Some(
                        OptionRow::new(&repo.full_name, &ui::FOLDER_GIT2, action)
                            .detail(&repo.default_branch)
                            .badges(if repo.private {
                                vec!["Private"]
                            } else {
                                Vec::new()
                            }),
                    )
                })
                .collect(),
            _ => Vec::new(),
        },
        View::Settings(project) => model
            .cache()
            .project(project)
            .map_or_else(Vec::new, settings::options),
        View::Icon(project) => settings::icons(project),
        View::Color { project, glyph } => settings::colors(project, glyph),
        _ => Vec::new(),
    };
    if !matches!(page.view, View::Folders(_) | View::Repositories) {
        let filter = page.query.trim().to_lowercase();
        options.retain(|item| {
            matches_filter(&item.title, &filter)
                || item
                    .subtitle
                    .as_ref()
                    .is_some_and(|value| matches_filter(value, &filter))
        });
    }
    options
}

pub fn clone_destination(url: String) -> Option<Action> {
    crate::model::projects::clone_destination(url)
}

fn folder_options(page: &ProjectPage) -> Vec<OptionRow> {
    let Some(ProjectReply::Folders(data)) = &page.data else {
        return Vec::new();
    };
    data.entries
        .iter()
        .map(|entry| {
            let item = OptionRow::new(&entry.name, &ui::FOLDER, Action::Browse(entry.path.clone()))
                .badges(folder_badges(entry.git, entry.project));
            if let Some(issue) = entry.issue {
                let mut item = item.detail(issue_label(issue)).disabled(true);
                item.notice = true;
                item
            } else {
                item
            }
        })
        .collect()
}

fn folder_badges(git: bool, project: bool) -> Vec<&'static str> {
    [(git, "Git"), (project, "Project")]
        .into_iter()
        .filter_map(|(show, label)| show.then_some(label))
        .collect()
}

const fn issue_label(issue: FolderIssue) -> &'static str {
    match issue {
        FolderIssue::PermissionDenied => "Permission denied",
        FolderIssue::NotFound => "Folder does not exist",
        FolderIssue::NotDirectory => "Path is not a directory",
        FolderIssue::Unavailable => "Folder is unavailable",
    }
}

fn session_options(page: &ProjectPage, harness: &str, project: Option<&str>) -> Vec<OptionRow> {
    let Some(ProjectReply::Sessions(result)) = &page.data else {
        return Vec::new();
    };
    result
        .sessions
        .iter()
        .map(|session| {
            let action = session.thread.map_or_else(
                || {
                    Action::Import(ImportSession {
                        harness: harness.into(),
                        session: session.id.clone(),
                        project: project.map(str::to_owned),
                        title: session.title.clone(),
                    })
                },
                Action::Open,
            );
            let detail = session.updated_at.as_ref().map_or_else(
                || session.cwd.clone(),
                |time| format!("{}  ·  {time}", session.cwd),
            );
            OptionRow::new(
                session.title.as_deref().unwrap_or(&session.id),
                ui::harness_icon(harness),
                action,
            )
            .detail(detail)
            .badges(if session.thread.is_some() {
                vec!["Imported"]
            } else {
                Vec::new()
            })
        })
        .collect()
}

pub const fn submit(page: &ProjectPage) -> Option<(&'static str, Action)> {
    Some(match &page.view {
        View::Folders(destination) if matches!(&page.data, Some(ProjectReply::Folders(data)) if data.issue.is_none()) => {
            (
                match destination {
                    Destination::Add if matches!(&page.data, Some(ProjectReply::Folders(data)) if data.project) => {
                        "Open project"
                    }
                    Destination::Add => "Add project",
                    Destination::Clone { .. } => "Clone",
                    Destination::Additional(_) => "Use folder",
                    Destination::NewProject => "Choose location",
                },
                Action::SubmitFolder,
            )
        }
        View::NewFolder | View::NewProject(_) => ("Create", Action::Submit),
        View::CloneUrl => ("Continue", Action::Submit),
        View::Edit { .. } => ("Save", Action::Submit),
        View::Remove(_) => ("Remove project", Action::Submit),
        _ => return None,
    })
}

fn button(label: &str, action: Action, disabled: bool, t: &Theme) -> El {
    let variant = match label {
        "Remove project" => rune_kit::ButtonVariant::Danger,
        "Add project" | "Clone" | "Save" | "Create" | "Continue" | "Use folder" => {
            rune_kit::ButtonVariant::Submit
        }
        _ => rune_kit::ButtonVariant::Outline,
    };
    ui::button(label, variant, Size::Compact)
        .disabled(disabled)
        .on_press_tx(move |s: &mut Desktop, tx| {
            s.dispatch(Msg::Palette(PaletteMsg::Project(action.clone())), tx);
        })
        .build(&t.overlay)
}

fn status(message: &str, t: &Theme) -> El {
    text(message)
        .font(t.small)
        .color(t.muted)
        .wrap()
        .role("status")
}

pub fn context(s: &Desktop, page: &ProjectPage, count: usize, t: &Theme) -> El {
    let mut body = col().gap(S8).px(S16);
    if let Some(error) = &page.error {
        body = body.child(
            text(error)
                .font(t.small)
                .color(t.danger)
                .wrap()
                .role("alert"),
        );
    }
    if page.pending && !matches!(page.data, Some(ProjectReply::Clone(_))) {
        body = body.child(status("Working…", t));
    }
    match (&page.view, &page.data) {
        (View::Folders(destination), Some(ProjectReply::Folders(data))) => {
            if let Destination::Clone { name, .. } = destination {
                body = body.child(status(&format!("Clone into {}/{}", data.path.trim_end_matches('/'), name), t));
            }
            let mut location = row().gap(S8).center_y().child(text(&data.path).font(t.mono_small).color(t.muted).truncate().grow(1.0).min_w(0.0));
            for tag in folder_badges(data.git, data.project) { location = location.child(ui::badge(tag).build(t)); }
            body = body.child(location);
            let mut tools = row().gap(S4);
            for (label, path) in [("Home", &data.home), ("Up", &data.parent)] {
                if let Some(path) = path { tools = tools.child(button(label, Action::Browse(path.clone()), page.pending, t)); }
            }
            tools = tools.child(button(if page.show_hidden { "Hide hidden" } else { "Show hidden" }, Action::Hidden, page.pending, t));
            if data.issue.is_none() {
                tools = tools.child(button("New folder", push(View::NewFolder), page.pending, t));
                if !data.git && *destination == Destination::Add {
                    tools = tools.child(button("Initialize Git", Action::Init, page.pending, t));
                }
            }
            body = body.child(tools);
            if let Some(issue) = data.issue { body = body.child(status(issue_label(issue), t)); }
            else if count == 0 { body = body.child(status("No matching directories", t)); }
        }
        (View::Cloning, Some(ProjectReply::Clone(clone))) => {
            match &clone.state {
                CloneState::Running { phase, percent } => {
                    let phase = match phase {
                        viz_client::protocol::ClonePhase::Preparing => "Preparing",
                        viz_client::protocol::ClonePhase::Counting => "Counting objects",
                        viz_client::protocol::ClonePhase::Compressing => "Compressing objects",
                        viz_client::protocol::ClonePhase::Receiving => "Receiving objects",
                        viz_client::protocol::ClonePhase::Resolving => "Resolving deltas",
                        viz_client::protocol::ClonePhase::CheckingOut => "Checking out files",
                    };
                    body = body.child(status(&percent.map_or_else(|| phase.into(), |pct| format!("{phase} · {pct}%")), t));
                    if let Some(percent) = percent {
                        body = body.child(row().h(S4).w_full().radius(S4).fill(t.selected_subtle)
                            .child(row().h(S4).w_frac(f32::from(*percent) / 100.0).radius(S4).fill(t.accent)));
                    }
                    body = body.child(button("Cancel clone", Action::CancelClone, false, t));
                }
                CloneState::Completed { project } => body = body.child(status(&format!("Cloned {project}"), t)),
                CloneState::Failed { message } => body = body.child(status(message, t)),
                CloneState::Cancelled => body = body.child(status("Clone cancelled", t)),
            }
        }
        (View::Settings(project) | View::Icon(project), _) => {
            if let Some(project) = s.model.cache().project(project) {
                body = body.child(settings::preview(project, s.project_icon_image(project.path.as_str()), t));
            }
        }
        (View::NewProject(parent), _) => body = body.child(status(&format!("Create in {parent}"), t)),
        (View::Remove(project), _) => body = body.child(status(&format!("Remove {project} from viz? The folder remains on disk."), t)),
        (View::Sessions { .. }, Some(ProjectReply::Sessions(result))) if !result.supported => body = body.child(status("This harness cannot list saved sessions.", t)),
        (View::Sessions { .. }, Some(ProjectReply::Sessions(result))) if result.sessions.is_empty() => body = body.child(status("No saved sessions", t)),
        (View::Repositories, Some(ProjectReply::Repositories(repos))) if repos.is_empty() => body = body.child(status("No matching repositories", t)),
        (View::Edit { field: crate::model::projects::Field::Options, .. }, _) => body = body.child(status("Use a JSON object with option IDs and string or boolean values. Clear the field to use harness defaults.", t)),
        _ => {}
    }
    body
}

pub fn footer(page: &ProjectPage, t: &Theme) -> Option<El> {
    if let Some((label, action)) = submit(page) {
        return Some(button(label, action, page.pending, t));
    }
    if matches!(&page.data, Some(ProjectReply::Sessions(result)) if result.next.is_some()) {
        return Some(button("Load more", Action::MoreSessions, page.pending, t));
    }
    None
}

fn sources() -> Vec<OptionRow> {
    vec![
        OptionRow::new(
            "New project",
            &ui::FOLDER_GIT2,
            push(View::Folders(Destination::NewProject)),
        )
        .detail("Create a folder and initialize Git"),
        OptionRow::new(
            "Local folder",
            &ui::FOLDER_OPEN,
            push(View::Folders(Destination::Add)),
        )
        .detail("Browse folders on the connected environment"),
        OptionRow::new("Git URL", &ui::GIT_BRANCH, push(View::CloneUrl))
            .detail("Clone a repository from a URL"),
        OptionRow::new(
            "GitHub repository",
            &ui::FOLDER_GIT2,
            push(View::Repositories),
        )
        .detail("Search repositories available to your GitHub account"),
    ]
}
