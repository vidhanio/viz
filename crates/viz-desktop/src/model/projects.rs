//! View stack and server results for project workflows.

use viz_client::protocol::{
    CloneStatus, FolderEntries, GitHubRepository, HarnessSessions, Project, ProjectSettingsPatch,
    ThreadId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    Add,
    Clone { url: String, name: String },
    Additional(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Name,
    Harness,
    Model,
    Options,
    Base,
    Prefix,
    Settle,
    Emoji,
}

impl Field {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Name => "Display name",
            Self::Harness => "Default harness ID",
            Self::Model => "Default model ID",
            Self::Options => "Default options (JSON)",
            Self::Base => "Default base branch",
            Self::Prefix => "Branch prefix",
            Self::Settle => "Auto-settle after days",
            Self::Emoji => "Emoji",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectView {
    Sources,
    Folders(Destination),
    NewFolder,
    CloneUrl,
    Repositories,
    Cloning,
    Projects,
    Settings(String),
    Edit {
        project: String,
        field: Field,
    },
    Icon(String),
    Color {
        project: String,
        glyph: String,
    },
    Remove(String),
    Harnesses,
    Sessions {
        harness: String,
        project: Option<String>,
    },
    Environments,
}

impl ProjectView {
    pub const fn title(&self) -> &'static str {
        match self {
            Self::Sources => "Add project",
            Self::Folders(Destination::Clone { .. }) => "Choose clone destination",
            Self::Folders(Destination::Additional(_)) => "Choose additional directory",
            Self::Folders(Destination::Add) => "Browse folders",
            Self::NewFolder => "New folder",
            Self::CloneUrl => "Clone from Git URL",
            Self::Repositories => "GitHub repositories",
            Self::Cloning => "Clone progress",
            Self::Projects => "Projects",
            Self::Settings(_) => "Project settings",
            Self::Edit { field, .. } => field.label(),
            Self::Icon(_) => "Choose project icon",
            Self::Color { .. } => "Choose icon color",
            Self::Remove(_) => "Remove project",
            Self::Harnesses => "Import harness session",
            Self::Sessions { .. } => "Saved sessions",
            Self::Environments => "Switch environment",
        }
    }

    pub const fn placeholder(&self) -> &'static str {
        match self {
            Self::Folders(_) => "Server directory path",
            Self::NewFolder => "Folder name",
            Self::CloneUrl => "https://github.com/owner/repo.git",
            Self::Edit { field, .. } => field.label(),
            _ => "Filter…",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProjectReply {
    Folders(FolderEntries),
    Repositories(Vec<GitHubRepository>),
    Sessions(HarnessSessions),
    Clone(CloneStatus),
    Directory(String),
    Added(Project),
    Updated(Project),
    Removed(String),
    Promoted {
        project: Project,
        threads: Vec<ThreadId>,
    },
    Imported(ThreadId),
    UploadCancelled,
}

#[derive(Debug, Clone)]
pub struct ProjectPage {
    pub view: ProjectView,
    pub query: String,
    pub data: Option<ProjectReply>,
    pub error: Option<String>,
    pub pending: bool,
    pub write_pending: bool,
    pub show_hidden: bool,
    pub request: u64,
    pub active: Option<usize>,
}

impl ProjectPage {
    pub const fn new(view: ProjectView, query: String) -> Self {
        Self {
            view,
            query,
            data: None,
            error: None,
            pending: false,
            write_pending: false,
            show_hidden: false,
            request: 0,
            active: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectPages {
    pub stack: Vec<ProjectPage>,
    pub revision: u64,
    pub request: u64,
    pub backwards: bool,
    pub due: Option<f64>,
    pub clock: f64,
}

#[derive(Debug, Clone)]
pub enum ProjectAction {
    Push(ProjectView, String),
    Browse(String),
    SubmitFolder,
    Init,
    Hidden,
    Submit,
    Save(String, Box<ProjectSettingsPatch>),
    Import(viz_client::protocol::ImportSession),
    Open(ThreadId),
    CancelClone,
    MoreSessions,
    Switch(String),
    UploadIcon(String),
}

pub fn clone_destination(url: String) -> Option<ProjectAction> {
    let name = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()?
        .trim_end_matches(".git");
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['\\', '?', '#']) {
        return None;
    }
    Some(ProjectAction::Push(
        ProjectView::Folders(Destination::Clone {
            name: name.to_owned(),
            url,
        }),
        String::new(),
    ))
}
