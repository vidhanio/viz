//! Execute project palette requests on the selected environment.

use anyhow::Context as _;
use viz_client::{
    Client,
    protocol::{
        BrowseFolders, CancelClone, CloneId, CloneState, CreateFolder, CreateProject,
        GitHubRepositories, ImportSession, InitRepository, ListHarnessSessions, ProjectIconChange,
        ProjectSettingsPatch, PromoteThread, ReadClone, ReadProject, RemoveProject, StartClone,
        UpdateProject,
    },
};

use super::server::Session;
use crate::{model::projects::ProjectReply, update::ServerMsg};

#[derive(Debug)]
pub enum ProjectRequest {
    Browse(BrowseFolders),
    Repositories(GitHubRepositories),
    Sessions(ListHarnessSessions),
    Import(ImportSession),
    Clone(StartClone),
    CloneStatus(CloneId),
    CancelClone(CloneId),
    CreateFolder(String),
    CreateGitProject(String),
    Init(String),
    Add(String),
    Save {
        project: String,
        name: Option<String>,
        patch: Box<ProjectSettingsPatch>,
    },
    Remove(String),
    Promote(PromoteThread),
    UploadIcon(String),
}

impl Session<'_> {
    pub(super) fn dispatch(&self, msg: ServerMsg) {
        super::server::dispatch_ui(self.ui, msg);
    }

    pub(super) fn project_palette(&self, request: u64, command: ProjectRequest) {
        let client = self.client.clone();
        let ui = self.ui.clone();
        let environment = self.environment.to_string();
        tokio::spawn(async move {
            let result = execute(&client, command)
                .await
                .map_err(|error| error.to_string());
            super::server::dispatch_ui(
                &ui,
                ServerMsg::ProjectPalette {
                    environment,
                    request,
                    result: Box::new(result),
                },
            );
        });
    }
}

async fn execute(client: &Client, command: ProjectRequest) -> anyhow::Result<ProjectReply> {
    let reply = match command {
        ProjectRequest::Browse(args) => ProjectReply::Folders(client.browse_folders(&args).await?),
        ProjectRequest::Repositories(args) => {
            ProjectReply::Repositories(client.github_repositories(&args).await?.repositories)
        }
        ProjectRequest::Sessions(args) => ProjectReply::Sessions(client.request(&args).await?),
        ProjectRequest::Import(args) => ProjectReply::Imported(client.request(&args).await?.thread),
        ProjectRequest::Clone(args) => ProjectReply::Clone(client.start_clone(&args).await?),
        ProjectRequest::CloneStatus(id) => {
            ProjectReply::Clone(client.clone_status(&ReadClone { id }).await?)
        }
        ProjectRequest::CancelClone(id) => {
            ProjectReply::Clone(client.cancel_clone(&CancelClone { id }).await?)
        }
        ProjectRequest::CreateGitProject(path) => create_git_project(client, path).await?,
        ProjectRequest::CreateFolder(path) => {
            ProjectReply::Directory(client.create_folder(&CreateFolder { path }).await?.path)
        }
        ProjectRequest::Init(path) => {
            ProjectReply::Directory(client.init_repository(&InitRepository { path }).await?.path)
        }
        ProjectRequest::Add(path) => ProjectReply::Added(
            client
                .create_project(&CreateProject { path, name: None })
                .await?,
        ),
        ProjectRequest::Save {
            project,
            name,
            patch,
        } => save(client, &project, name.as_deref(), *patch).await?,
        ProjectRequest::Remove(project) => {
            client
                .remove_project(&RemoveProject {
                    project: project.clone(),
                })
                .await?;
            ProjectReply::Removed(project)
        }
        ProjectRequest::Promote(args) => {
            let reply = client.promote_thread(&args).await?;
            ProjectReply::Promoted {
                project: reply.project,
                threads: reply.threads,
            }
        }
        ProjectRequest::UploadIcon(project) => {
            let icon = tokio::task::spawn_blocking(super::platform::pick_project_icon)
                .await
                .map_err(std::io::Error::other)??;
            let Some(icon) = icon else {
                return Ok(ProjectReply::UploadCancelled);
            };
            save(
                client,
                &project,
                None,
                ProjectSettingsPatch {
                    icon: Some(icon),
                    ..Default::default()
                },
            )
            .await?
        }
    };
    if let ProjectReply::Clone(status) = &reply
        && let CloneState::Completed { project } = &status.state
    {
        let result = client
            .request(&ReadProject {
                project: project.clone(),
            })
            .await?;
        return Ok(ProjectReply::Added(result.project));
    }
    Ok(reply)
}

async fn save(
    client: &Client,
    project: &str,
    name: Option<&str>,
    patch: ProjectSettingsPatch,
) -> anyhow::Result<ProjectReply> {
    client
        .update_project(&UpdateProject {
            project: project.to_owned(),
            name: name.map(str::to_owned),
            changes: Some(patch),
        })
        .await?;
    Ok(ProjectReply::Updated(
        client
            .request(&ReadProject {
                project: project.to_owned(),
            })
            .await?
            .project,
    ))
}

pub(super) fn uploaded_icon(file: viz_client::protocol::Attachment) -> ProjectIconChange {
    ProjectIconChange::Uploaded {
        media_type: file.mime,
        data: file.data,
    }
}

async fn create_git_project(client: &Client, path: String) -> anyhow::Result<ProjectReply> {
    let folder = client.create_folder(&CreateFolder { path }).await?;
    let folder = client
        .init_repository(&InitRepository {
            path: folder.path.clone(),
        })
        .await
        .with_context(|| {
            format!(
                "Folder created at {}. Initialize Git there to continue",
                folder.path
            )
        })?;
    let result = client
        .create_project(&CreateProject {
            path: folder.path.clone(),
            name: None,
        })
        .await
        .with_context(|| {
            format!(
                "Git initialized at {}. Add that folder to finish",
                folder.path
            )
        })?;
    Ok(ProjectReply::Added(result))
}
