//! Preview project palette routes with representative server results.

use viz_client::protocol::{
    CloneId, ClonePhase, CloneState, CloneStatus, FolderEntries, FolderEntry, FolderIssue,
    GitHubRepository, HarnessSession, HarnessSessions, ThreadId,
};

use crate::{
    app::Desktop,
    model::projects::{Destination, ProjectPage, ProjectReply, ProjectView},
};

const PROJECT: &str = "/work/workbench";

fn preview(s: &mut Desktop, view: ProjectView, query: &str, data: Option<ProjectReply>) {
    super::fixtures::seed_workspace(&mut s.model);
    s.model.palette.open = true;
    let mut page = ProjectPage::new(view, query.into());
    page.data = data;
    s.model.palette.pages.stack.push(page);
    s.sync_widgets_from_model();
}

pub(super) fn sources(s: &mut Desktop) {
    preview(s, ProjectView::Sources, "", None);
}

pub(super) fn folders(s: &mut Desktop) {
    let entries = [
        ("workbench", true, true, None),
        ("platform", true, false, None),
        ("prototypes", false, false, None),
        ("shared", false, false, Some(FolderIssue::PermissionDenied)),
    ]
    .map(|(name, git, project, issue)| FolderEntry {
        path: format!("/work/{name}"),
        name: name.into(),
        git,
        project,
        issue,
    });
    preview(
        s,
        ProjectView::Folders(Destination::Add),
        "/work/",
        Some(ProjectReply::Folders(FolderEntries {
            path: "/work".into(),
            parent: Some("/".into()),
            home: Some("/home/developer".into()),
            git: false,
            project: false,
            completion: None,
            issue: None,
            entries: entries.into(),
        })),
    );
}

pub(super) fn new_folder(s: &mut Desktop) {
    folders(s);
    s.model
        .palette
        .pages
        .stack
        .push(ProjectPage::new(ProjectView::NewFolder, "prototype".into()));
    s.sync_widgets_from_model();
}

pub(super) fn repositories(s: &mut Desktop) {
    let repositories = ["platform", "prototypes", "workbench"].map(|name| GitHubRepository {
        name: name.into(),
        full_name: format!("studio/{name}"),
        default_branch: "main".into(),
        clone_url: format!("https://github.com/studio/{name}.git"),
        ssh_url: String::new(),
        avatar_url: String::new(),
        private: name == "workbench",
    });
    preview(
        s,
        ProjectView::Repositories,
        "studio",
        Some(ProjectReply::Repositories(repositories.into())),
    );
}

pub(super) fn cloning(s: &mut Desktop) {
    preview(
        s,
        ProjectView::Cloning,
        "",
        Some(ProjectReply::Clone(CloneStatus {
            id: CloneId("preview".into()),
            state: CloneState::Running {
                phase: ClonePhase::Receiving,
                percent: Some(64),
            },
        })),
    );
}

pub(super) fn settings(s: &mut Desktop) {
    preview(s, ProjectView::Settings(PROJECT.into()), "", None);
}

pub(super) fn icon(s: &mut Desktop) {
    preview(s, ProjectView::Icon(PROJECT.into()), "", None);
}

pub(super) fn colors(s: &mut Desktop) {
    preview(
        s,
        ProjectView::Color {
            project: PROJECT.into(),
            glyph: "folder-code".into(),
        },
        "",
        None,
    );
}

pub(super) fn sessions(s: &mut Desktop) {
    preview(
        s,
        ProjectView::Sessions {
            harness: "codex".into(),
            project: Some(PROJECT.into()),
        },
        "",
        Some(ProjectReply::Sessions(HarnessSessions {
            supported: true,
            next: Some("next".into()),
            sessions: vec![
                HarnessSession {
                    id: "current".into(),
                    cwd: PROJECT.into(),
                    title: Some("Update workspace provisioning".into()),
                    updated_at: Some("2026-11-22 17:42".into()),
                    thread: None,
                },
                HarnessSession {
                    id: "previous".into(),
                    cwd: PROJECT.into(),
                    title: Some("Review folder permissions".into()),
                    updated_at: Some("2026-11-21 09:13".into()),
                    thread: Some(ThreadId(1)),
                },
            ],
        })),
    );
}
