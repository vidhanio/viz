//! Public workspace methods bind deserialized parameters to typed operations.

use std::{num::NonZeroU64, path::Path, sync::Arc, time::Duration};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use viz_core::threads::Patch;
use viz_protocol::{
    BranchList, ConfigureThread, CreateProject, DeleteThread, Empty, FileList, HarnessList,
    HarnessOptions, HarnessOptionsResult, ListBranches, ListProjects, ListWorktrees, MoveThread,
    ProjectList, ProjectResult, ReadProject, ReadThreadState, RemoveProject, RemoveQueuedInput,
    RemoveWorktree, SearchFiles, SearchHits, SearchThreads, SendInput, SteerQueuedInput,
    StopThread, ThreadList, ThreadStateResult, UpdateProject, UpdateQueuedInput, UpdateThread,
    WaitThread,
};

use super::{
    Workspace, runs,
    sql::{exec, row},
    store, worktree,
};
use crate::{
    rpc::handlers,
    wire::{OptionValue, RpcError},
};

type Id = NonZeroU64;

handlers! {
    ROUTES for Arc<Workspace> {
        viz_protocol::ListThreads<'static>, ListThreads, |workspace, ListThreads { project, archived }| {
            let threads = store::list_threads(&mut *workspace.db.lock().await, archived, project.as_deref(), workspace).await?;
            Ok(ThreadList { threads })
        }
        viz_protocol::ListEntries, Entries, |workspace, Entries { thread, before, limit }| {
            workspace.entries(thread.get(), before.map(Id::get), page_limit(limit, 100)?).await
        }
        ReadThreadState, Thread, |workspace, Thread { thread }| {
            Ok(ThreadStateResult { state: workspace.state(thread.get()).await? })
        }
        SearchThreads<'static>, Search, |workspace, Search { query, limit }| {
            let hits = store::search(&mut *workspace.db.lock().await, &query, page_limit(limit, 50)?).await?;
            Ok(SearchHits { hits })
        }
        SearchFiles<'static>, FileSearch, |workspace, FileSearch { thread, query, limit }| {
            let cwd = row(&mut *workspace.db.lock().await, "SELECT cwd FROM threads WHERE id = ?1",
                [thread.get().into()]).await?.ok_or_else(|| store::missing(thread.get()))?.str(0)?;
            let files = workspace.files.search(&cwd, &query, page_limit(limit, 50)?).await;
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
        UpdateProject<'static>, ProjectUpdate, |workspace, ProjectUpdate { project, name }| {
            let mut db = workspace.db.lock().await;
            store::project(&mut db, &project).await?;
            if let Some(name) = name {
                if name.is_empty() {
                    return Err(RpcError::invalid("project name must not be empty"));
                }
                exec(&mut db, "UPDATE projects SET name = ?2 WHERE path = ?1",
                    [project.into(), name.into()]).await?;
                drop(db);
                workspace.changed(None);
            }
            Ok(Empty {})
        }
        RemoveProject<'static>, ProjectKey, |workspace, ProjectKey { project }| {
            workspace.remove_project(&project).await?;
            Ok(Empty {})
        }
        ListBranches<'static>, ProjectKey, |workspace, ProjectKey { project }| {
            let project = store::project(&mut *workspace.db.lock().await, &project).await?;
            let branches = worktree::branches(Path::new(&project.path)).await?;
            Ok(BranchList { branches })
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
        SendInput, SendInput, |workspace, input| {
            if input.origin.is_some() {
                return Err(RpcError::invalid("origins require a connector WebSocket"));
            }
            workspace.send(input).await
        }
        WaitThread, Wait, |workspace, Wait { thread, run, timeout_ms }| {
            let timeout = timeout_ms.unwrap_or(30_000);
            if timeout > 120_000 {
                return Err(RpcError::invalid("timeoutMs must not exceed 120000"));
            }
            workspace.wait_run(thread.get(), run.map(Id::get), Duration::from_millis(timeout)).await
        }
        StopThread, ThreadRun, |workspace, ThreadRun { thread, run }| {
            workspace.stop(thread.get(), run.map(Id::get)).await?;
            Ok(Empty {})
        }
        UpdateThread<'static>, UpdateMetadata, |workspace, input| {
            workspace.update(input).await?;
            Ok(Empty {})
        }
        ConfigureThread<'static>, Configure, |workspace, Configure { thread, option, value }| {
            workspace.configure(thread.get(), &option, value).await?;
            Ok(Empty {})
        }
        DeleteThread, DeleteThread, |workspace, input| {
            workspace.delete(input.thread.0, input.remove_worktree).await?;
            Ok(Empty {})
        }
        RemoveQueuedInput, QueueRun, |workspace, QueueRun { thread, run }| {
            let mut db = workspace.db.lock().await;
            store::require(&mut db, thread.get()).await?;
            runs::remove_queued(&mut db, thread.get(), run.get()).await?;
            drop(db);
            workspace.settled(thread.get());
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
        viz_protocol::ListHarnesses, Empty, |workspace, _| {
            Ok(HarnessList { harnesses: workspace.list_harnesses().await })
        }
        HarnessOptions, HarnessOptions, |workspace, HarnessOptions { harness, model }| {
            let options = workspace.harness_options(&harness, Some(&model)).await?;
            Ok(HarnessOptionsResult { options })
        }
    }
}

impl Workspace {
    async fn update_queue(&self, input: UpdateQueue) -> Result<(), RpcError> {
        let UpdateQueue {
            thread,
            run,
            before,
            text,
        } = input;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread.get()).await?;
        runs::update_queue(&mut tx, thread.get(), run.get(), text.as_deref(), before).await?;
        tx.commit().await?;
        drop(db);
        self.changed(Some(thread.get()));
        Ok(())
    }

    async fn steer_queue(self: &Arc<Self>, input: SteerQueuedInput) -> Result<(), RpcError> {
        let thread = input.thread.0;
        let mut db = self.db.lock().await;
        let mut tx = db.transaction().await?;
        store::require(&mut tx, thread).await?;
        let active = runs::active(&mut tx, thread).await?;
        runs::promote_queued(&mut tx, thread, input.run.0).await?;
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

fn page_limit(limit: Option<usize>, default: usize) -> Result<usize, RpcError> {
    match limit {
        Some(1..=200) | None => Ok(limit.unwrap_or(default)),
        _ => Err(RpcError::invalid("limit must be between 1 and 200")),
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
pub(crate) struct Thread {
    pub(crate) thread: Id,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ThreadRun {
    pub(crate) thread: Id,
    pub(crate) run: Option<Id>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueueRun {
    pub(crate) thread: Id,
    pub(crate) run: Id,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Wait {
    pub(crate) thread: Id,
    pub(crate) run: Option<Id>,
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct Entries {
    pub(crate) thread: Id,
    pub(crate) before: Option<Id>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct Search {
    pub(crate) query: String,
    #[schemars(range(min = 1, max = 200))]
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileSearch {
    pub(crate) thread: Id,
    pub(crate) query: String,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Configure {
    pub(crate) thread: Id,
    pub(crate) option: String,
    pub(crate) value: OptionValue,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectKey {
    pub(crate) project: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectCreate {
    pub(crate) path: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct ProjectUpdate {
    pub(crate) project: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateMetadata {
    pub(crate) thread: Id,
    pub(crate) title: Option<String>,
    pub(crate) archived: Option<bool>,
    pub(crate) pinned: Option<bool>,
    pub(crate) unread: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    pub(crate) project: Patch<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateQueue {
    pub(crate) thread: Id,
    pub(crate) run: Id,
    pub(crate) text: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub(crate) before: Patch<Id>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) struct WorktreeRemove {
    /// Absolute path of the worktree root.
    pub(crate) path: String,
    /// Discard uncommitted changes and untracked files.
    #[serde(default)]
    pub(crate) force: bool,
}
