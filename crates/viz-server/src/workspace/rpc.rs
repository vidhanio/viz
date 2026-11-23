//! Public workspace methods bind deserialized parameters to typed operations.

use std::{path::Path, sync::Arc, time::Duration};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use viz_core::threads::{Changes, Patch};
use viz_protocol::{
    BranchList, BrowseFolders, CancelClone, ConfigureThread, CreateFolder, CreateProject,
    DeleteThread, Empty, FileList, ForkThread, GitHubRepositories, HarnessList, HarnessOptions,
    HarnessOptionsResult, InitRepository, ListBranches, ListEntries, ListProjects, ListWorktrees,
    MoveThread, ProjectList, ProjectResult, PromoteThread, ReadClone, ReadEntryChunk, ReadProject,
    ReadProjectIcon, ReadThreadState, RemoveProject, RemoveQueuedInput, RemoveWorktree,
    RetitleThread, RunId, SearchFiles, SearchHits, SearchThreads, SendInput, StartClone,
    SteerQueuedInput, StopThread, ThreadId, ThreadList, ThreadStateResult, TriageThread,
    UpdateProject, UpdateQueuedInput, UpdateThread, WaitThread,
};

use super::{Workspace, runs, sql::row, store};
use crate::{error::Error, rpc::handlers, wire::OptionValue};

handlers! {
    ROUTES for Arc<Workspace> {
        viz_protocol::ListThreads<'static>, ListThreads, |workspace, ListThreads { project, archived }| {
            let threads = store::list_threads(&mut *workspace.db.lock().await, archived, project.as_deref(), workspace).await?;
            Ok(ThreadList { threads })
        }
        ListEntries, ListEntries, |workspace, ListEntries { thread, before, limit }| {
            workspace.entries(thread, before, page_limit(limit, 100)?).await
        }
        ReadEntryChunk, ReadEntryChunk, |workspace, request| {
            store::chunk(workspace, request).await
        }
        ReadThreadState, ReadThreadState, |workspace, ReadThreadState { thread }| {
            Ok(ThreadStateResult { state: workspace.state(thread).await? })
        }
        SearchThreads<'static>, Search, |workspace, Search { query, limit }| {
            let hits = store::search(&mut *workspace.db.lock().await, &query, page_limit(limit, 50)?).await?;
            Ok(SearchHits { hits })
        }
        SearchFiles<'static>, FileSearch, |workspace, FileSearch { thread, project, query, limit }| {
            let cwd = match (thread, project) {
                (Some(thread), None) => row(&mut *workspace.db.lock().await,
                    "SELECT cwd FROM threads WHERE id = ?1", [thread.into()]
                ).await?.ok_or_else(|| Error::NoThread(thread))?.str(0)?,
                (None, Some(project)) => {
                    store::preferences(&mut *workspace.db.lock().await, &project).await?;
                    project
                }
                _ => return Err(Error::invalid("choose one file search target")),
            };
            let files = viz_git::find_files(
                Path::new(&cwd), &query, page_limit(limit, 50)?, &workspace.closing
            ).await?.into_iter().map(|found| viz_protocol::FileSearchMatch {
                path: found.path.to_string_lossy().into_owned(),
                kind: match found.kind {
                    viz_git::PathKind::File => viz_protocol::FileSearchKind::File,
                    viz_git::PathKind::Directory => viz_protocol::FileSearchKind::Directory,
                },
                score: found.score,
                positions: found.positions,
            }).collect();
            Ok(FileList { files })
        }
        ListProjects, Empty, |workspace, _| {
            let projects = store::projects(&mut *workspace.db.lock().await).await?;
            Ok(ProjectList { projects })
        }
        ReadProject<'static>, ProjectKey, |workspace, ProjectKey { project }| {
            let project = store::project(&mut *workspace.db.lock().await, &project).await?;
            Ok(ProjectResult { project })
        }
        CreateProject<'static>, ProjectCreate, |workspace, ProjectCreate { path, name }| {
            Ok(ProjectResult { project: workspace.create_project(&path, name).await? })
        }
        UpdateProject<'static>, ProjectUpdate, |workspace, ProjectUpdate { project, name, changes }| {
            workspace.update_project(&project, name, changes.unwrap_or_default()).await?;
            Ok(Empty {})
        }
        BrowseFolders, BrowseFolders, |workspace, args| { workspace.browse_folders(args).await }
        CreateFolder, CreateFolder, |workspace, args| { workspace.create_folder(args).await }
        viz_protocol::GitStatusRequest, viz_protocol::GitStatusRequest, |workspace, args| {
            let repo = workspace.git_repository(args.thread).await?;
            workspace.git_status(&repo).await
        }
        viz_protocol::GitActionRequest, viz_protocol::GitActionRequest, |workspace, args| {
            workspace.git_action(args).await
        }
        viz_protocol::RestoreCheckpoint, viz_protocol::RestoreCheckpoint, |workspace, args| {
            workspace.restore_checkpoint(args).await?;
            Ok(Empty {})
        }
        viz_protocol::GitDiffRequest, viz_protocol::GitDiffRequest, |workspace, args| {
            workspace.git_diff(args).await
        }
        InitRepository, InitRepository, |workspace, args| { workspace.init_repository(args).await }
        StartClone, StartClone, |workspace, args| { workspace.start_clone(args).await }
        ReadClone, ReadClone, |workspace, ReadClone { id }| { workspace.clone_status(id).await }
        CancelClone, CancelClone, |workspace, CancelClone { id }| { workspace.cancel_clone(id).await }
        viz_protocol::GitHubAuth, viz_protocol::GitHubAuth, |workspace, args| { workspace.github_auth(args).await }
        viz_protocol::GitHubSearch, viz_protocol::GitHubSearch, |workspace, args| { workspace.github_search(args).await }
        viz_protocol::GitHubChecks, viz_protocol::GitHubChecks, |workspace, args| { workspace.github_checks(args).await }
        viz_protocol::GitHubPullPage, viz_protocol::GitHubPullPage, |workspace, args| { workspace.github_reviews(args).await }
        viz_protocol::GitHubCreatePull, viz_protocol::GitHubCreatePull, |workspace, args| { workspace.github_create_pull(args).await }
        viz_protocol::GitHubLaunch, viz_protocol::GitHubLaunch, |workspace, args| { workspace.github_launch(args).await }
        GitHubRepositories, GitHubRepositories, |workspace, args| { workspace.github_repositories(args).await }
        PromoteThread, PromoteThread, |workspace, args| { workspace.promote_thread(args).await }
        ReadProjectIcon, ReadProjectIcon, |workspace, args| {
            workspace.project_icon(args).await
        }
        RemoveProject<'static>, ProjectKey, |workspace, ProjectKey { project }| {
            workspace.remove_project(&project).await?;
            Ok(Empty {})
        }
        ListBranches<'static>, BranchQuery, |workspace, BranchQuery { project, query, offset, limit }| {
            let project = store::project(&mut *workspace.db.lock().await, &project).await?;
            let query = query.unwrap_or_default().to_lowercase();
            let offset = offset.unwrap_or_default();
            let limit = page_limit(limit, 100)?;
            let repo = viz_git::Repository::discover(Path::new(&project.path), &workspace.closing).await?;
            let all = repo.branches(&workspace.closing).await?;
            let mut branches = all.entries.into_iter()
                .filter(|branch| branch.name.to_lowercase().contains(&query))
                .skip(offset).take(limit + 1).map(|branch| viz_protocol::GitBranch {
                    name: branch.name, remote: branch.remote, upstream: branch.upstream,
                    checked_out_elsewhere: branch.checked_out_elsewhere,
                }).collect::<Vec<_>>();
            let next = (branches.len() > limit).then(|| offset.saturating_add(limit));
            branches.truncate(limit);
            Ok(BranchList { branches, next, default: all.default })
        }
        ListWorktrees<'static>, ProjectKey, |workspace, ProjectKey { project }| {
            workspace.list_worktrees(&project).await
        }
        RemoveWorktree<'static>, WorktreeRemove, |workspace, WorktreeRemove { path, force }| {
            workspace.remove_worktree(&path, force).await?;
            Ok(Empty {})
        }
        MoveThread, MoveThread, |workspace, input| {
            workspace.move_thread(input).await
        }
        viz_protocol::LaunchThread, viz_protocol::LaunchThread, |workspace, input| {
            workspace.launch(input).await
        }
        ForkThread, ForkThread, |workspace, input| {
            workspace.fork(input).await
        }
        SendInput, SendInput, |workspace, input| {
            if input.origin.is_some() {
                return Err(Error::invalid("origins require a connector WebSocket"));
            }
            workspace.send(input).await
        }
        WaitThread, WaitThread, |workspace, WaitThread { thread, run, timeout_ms }| {
            let timeout = timeout_ms.unwrap_or(30_000);
            if timeout > 120_000 {
                return Err(Error::invalid("timeoutMs must not exceed 120000"));
            }
            workspace.wait_run(thread, run, Duration::from_millis(timeout)).await
        }
        StopThread, StopThread, |workspace, StopThread { thread, run }| {
            workspace.stop(thread, run).await?;
            Ok(Empty {})
        }
        UpdateThread<'static>, UpdateMetadata, |workspace, input| {
            workspace.update(input).await
        }
        RetitleThread, RetitleThread, |workspace, input| {
            workspace.retitle(input).await
        }
        TriageThread, TriageThread, |workspace, input| {
            workspace.triage(input).await
        }
        ConfigureThread<'static>, Configure, |workspace, Configure { thread, option, value }| {
            workspace.configure(thread, option, value).await?;
            Ok(Empty {})
        }
        DeleteThread, DeleteThread, |workspace, input| {
            workspace.delete(input.thread, input.remove_worktree).await?;
            Ok(Empty {})
        }
        RemoveQueuedInput, RemoveQueuedInput, |workspace, RemoveQueuedInput { thread, run }| {
            let mut db = workspace.db.lock().await;
            let mut tx = db.transaction().await?;
            store::require(&mut tx, thread).await?;
            runs::remove(&mut tx, thread, run).await?;
            tx.commit().await?;
            drop(db);
            workspace.settled(thread);
            Ok(Empty {})
        }
        UpdateQueuedInput<'static>, UpdateQueue, |workspace, input| {
            workspace.update_queue(input).await?;
            Ok(Empty {})
        }
        SteerQueuedInput, SteerQueuedInput, |workspace, input| {
            workspace.steer_queue(input).await?;
            Ok(Empty {})
        }
        viz_protocol::AnswerRequest, viz_protocol::AnswerRequest, |workspace, input| {
            workspace.answer(input, viz_protocol::AnsweredBy::User).await
        }
        viz_protocol::SetThreadAccess, viz_protocol::SetThreadAccess, |workspace, viz_protocol::SetThreadAccess { thread, access }| {
            workspace.set_thread_access(thread, access).await?;
            Ok(Empty {})
        }
        viz_protocol::SetProjectAccess, viz_protocol::SetProjectAccess, |workspace, input| {
            workspace.set_project_access(input).await?;
            Ok(Empty {})
        }
        viz_protocol::ListHarnesses, Empty, |workspace, _| {
            Ok(HarnessList { harnesses: workspace.list_harnesses().await? })
        }
        viz_protocol::ListHarnessSessions, viz_protocol::ListHarnessSessions, |workspace, input| {
            workspace.harness_sessions(input).await
        }
        viz_protocol::ImportSession, viz_protocol::ImportSession, |workspace, input| {
            workspace.import(input).await
        }
        HarnessOptions, HarnessOptions, |workspace, HarnessOptions { harness, model }| {
            let options = workspace.harness_options(&harness, Some(&model)).await?;
            Ok(HarnessOptionsResult { options })
        }
    }
}

impl Workspace {
    async fn update_queue(&self, input: UpdateQueue) -> Result<(), Error> {
        let UpdateQueue {
            thread,
            run,
            before,
            text,
        } = input;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        runs::update_queue(&mut tx, thread, run, text.as_deref(), before).await?;
        tx.commit().await?;
        drop(db);
        self.changed(Some(thread));
        Ok(())
    }

    async fn steer_queue(self: &Arc<Self>, input: SteerQueuedInput) -> Result<(), Error> {
        let thread = input.thread;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        let active = runs::active(&mut tx, thread).await?;
        runs::promote(&mut tx, thread, input.run).await?;
        if let Some(active) = active {
            store::withdraw_requests(&mut tx, Some(active), viz_protocol::Withdrawal::Steered)
                .await?;
        }
        tx.commit().await?;
        if let Some(active) = active {
            self.steer_active(thread, active);
        }
        self.kick(thread, false);
        drop(db);
        self.changed(Some(thread));
        Ok(())
    }
}

fn page_limit(limit: Option<u16>, default: usize) -> Result<usize, Error> {
    match limit {
        None => Ok(default),
        Some(limit @ 1..=200) => Ok(limit.into()),
        Some(_) => Err(Error::invalid("limit must be between 1 and 200")),
    }
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<Patch<T>, D::Error> {
    Option::deserialize(de).map(|value| value.map_or(Patch::Clear, Patch::Set))
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ListThreads {
    pub(crate) archived: Option<bool>,
    pub(crate) project: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct Search {
    pub(crate) query: String,
    #[schemars(range(min = 1, max = 200))]
    pub(crate) limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileSearch {
    pub(crate) thread: Option<ThreadId>,
    pub(crate) project: Option<String>,
    pub(crate) query: String,
    pub(crate) limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Configure {
    pub(crate) thread: ThreadId,
    pub(crate) option: String,
    pub(crate) value: OptionValue,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectKey {
    pub(crate) project: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct BranchQuery {
    project: String,
    query: Option<String>,
    offset: Option<usize>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectCreate {
    pub(crate) path: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectUpdate {
    pub(crate) changes: Option<viz_protocol::ProjectSettingsPatch>,
    pub(crate) project: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(from = "RawUpdate")]
pub(crate) struct UpdateMetadata {
    pub(crate) thread: ThreadId,
    pub(crate) changes: Changes,
}

#[derive(Deserialize)]
struct RawUpdate {
    thread: ThreadId,
    title: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    project: Patch<String>,
}

impl From<RawUpdate> for UpdateMetadata {
    fn from(raw: RawUpdate) -> Self {
        Self {
            thread: raw.thread,
            changes: Changes {
                title: raw.title,
                project: raw.project,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateQueue {
    pub(crate) thread: ThreadId,
    pub(crate) run: RunId,
    pub(crate) text: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub(crate) before: Patch<RunId>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct WorktreeRemove {
    /// Absolute path of the worktree root.
    pub(crate) path: String,
    /// Discard uncommitted changes and untracked files.
    #[serde(default)]
    pub(crate) force: bool,
}
