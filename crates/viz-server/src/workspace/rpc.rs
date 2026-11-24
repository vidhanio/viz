//! Public workspace methods bind deserialized parameters to typed operations.

use std::{path::Path, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::io::AsyncReadExt as _;
use viz_protocol::{
    AnswerRequest, BranchList, BrowseFolders, CancelClone, ConfigureThread, CreateFolder,
    CreateProject, DeleteThread, Empty, FileContent, FileList, ForkThread, GitActionRequest,
    GitDiffRequest, GitHubAuth, GitHubChecks, GitHubCreatePull, GitHubLaunch, GitHubPullPage,
    GitHubRepositories, GitHubSearch, GitStatusRequest, HarnessList, HarnessOptions,
    HarnessOptionsResult, ImportSession, InitRepository, LaunchThread, ListBranches, ListEntries,
    ListHarnessSessions, ListHarnesses, ListProjects, ListThreads, ListWorktrees, MoveThread,
    ProjectList, ProjectResult, PromoteThread, ReadAttachment, ReadClone, ReadEntryChunk, ReadFile,
    ReadProject, ReadProjectIcon, ReadThreadState, RemoveProject, RemoveQueuedInput,
    RemoveWorktree, RestoreCheckpoint, RetitleThread, SearchFiles, SearchHits, SearchThreads,
    SendInput, SetProjectAccess, SetThreadAccess, StartClone, SteerQueuedInput, StopThread,
    ThreadList, ThreadStateResult, TriageThread, UpdateProject, UpdateQueuedInput, UpdateThread,
    WaitThread,
};

use super::{Workspace, runs, sql::row, store};
use crate::{error::Error, rpc::handlers};

handlers! {
    ROUTES for Arc<Workspace> {
        ListThreads, |workspace, ListThreads { project, archived }| {
            let threads = store::list_threads(&mut *workspace.db.lock().await, archived, project.as_deref(), workspace).await?;
            Ok(ThreadList { threads })
        }
        ListEntries, |workspace, ListEntries { thread, before, limit }| {
            workspace.entries(thread, before, page_limit(limit, 500)?).await
        }
        ReadEntryChunk, |workspace, request| {
            store::chunk(workspace, request).await
        }
        ReadThreadState, |workspace, ReadThreadState { thread }| {
            Ok(ThreadStateResult { state: workspace.state(thread).await? })
        }
        SearchThreads, |workspace, SearchThreads { query, limit }| {
            let hits = store::search(&mut *workspace.db.lock().await, &query, page_limit(limit, 50)?).await?;
            Ok(SearchHits { hits })
        }
        SearchFiles, |workspace, SearchFiles { thread, project, query, limit }| {
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
        ReadFile, |workspace, args| {
            workspace.read_file_preview(args).await
        }
        ReadAttachment, |workspace, ReadAttachment { thread, id }| {
            let mut items = workspace.attachments(thread, &[id]).await?;
            items
                .pop()
                .ok_or_else(|| Error::invalid("attachment not found"))
        }
        ListProjects, |workspace, _| {
            let projects = store::projects(&mut *workspace.db.lock().await).await?;
            Ok(ProjectList { projects })
        }
        ReadProject, |workspace, ReadProject { project }| {
            let project = store::project(&mut *workspace.db.lock().await, &project).await?;
            Ok(ProjectResult { project })
        }
        CreateProject, |workspace, CreateProject { path, name }| {
            Ok(ProjectResult { project: workspace.create_project(&path, name).await? })
        }
        UpdateProject, |workspace, UpdateProject { project, name, changes }| {
            workspace.update_project(&project, name, changes.unwrap_or_default()).await?;
            Ok(Empty {})
        }
        BrowseFolders, |workspace, args| { workspace.browse_folders(args).await }
        CreateFolder, |workspace, args| { workspace.create_folder(args).await }
        GitStatusRequest, |workspace, args| {
            let repo = workspace.git_repository(args.thread).await?;
            workspace.git_status(&repo).await
        }
        GitActionRequest, |workspace, args| {
            workspace.git_action(args).await
        }
        RestoreCheckpoint, |workspace, args| {
            workspace.restore_checkpoint(args).await?;
            Ok(Empty {})
        }
        GitDiffRequest, |workspace, args| {
            workspace.git_diff(args).await
        }
        InitRepository, |workspace, args| { workspace.init_repository(args).await }
        StartClone, |workspace, args| { workspace.start_clone(args).await }
        ReadClone, |workspace, ReadClone { id }| { workspace.clone_status(id).await }
        CancelClone, |workspace, CancelClone { id }| { workspace.cancel_clone(id).await }
        GitHubAuth, |workspace, args| { workspace.github_auth(args).await }
        GitHubSearch, |workspace, args| { workspace.github_search(args).await }
        GitHubChecks, |workspace, args| { workspace.github_checks(args).await }
        GitHubPullPage, |workspace, args| { workspace.github_reviews(args).await }
        GitHubCreatePull, |workspace, args| { workspace.github_create_pull(args).await }
        GitHubLaunch, |workspace, args| { workspace.github_launch(args).await }
        GitHubRepositories, |workspace, args| { workspace.github_repositories(args).await }
        PromoteThread, |workspace, args| { workspace.promote_thread(args).await }
        ReadProjectIcon, |workspace, args| {
            workspace.project_icon(args).await
        }
        RemoveProject, |workspace, RemoveProject { project }| {
            workspace.remove_project(&project).await?;
            Ok(Empty {})
        }
        ListBranches, |workspace, ListBranches { project, query, offset, limit }| {
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
        ListWorktrees, |workspace, ListWorktrees { project }| {
            workspace.list_worktrees(&project).await
        }
        RemoveWorktree, |workspace, RemoveWorktree { path, force }| {
            workspace.remove_worktree(&path, force).await?;
            Ok(Empty {})
        }
        MoveThread, |workspace, input| {
            workspace.move_thread(input).await
        }
        LaunchThread, |workspace, input| {
            workspace.launch(input).await
        }
        ForkThread, |workspace, input| {
            workspace.fork(input).await
        }
        SendInput, |workspace, input| {
            if input.origin.is_some() {
                return Err(Error::invalid("origins require a connector WebSocket"));
            }
            workspace.send(input).await
        }
        WaitThread, |workspace, WaitThread { thread, run, timeout_ms }| {
            let timeout = timeout_ms.unwrap_or(30_000);
            if timeout > 120_000 {
                return Err(Error::invalid("timeoutMs must not exceed 120000"));
            }
            workspace.wait_run(thread, run, Duration::from_millis(timeout)).await
        }
        StopThread, |workspace, StopThread { thread, run }| {
            workspace.stop(thread, run).await?;
            Ok(Empty {})
        }
        UpdateThread, |workspace, input| {
            workspace.update(input).await
        }
        RetitleThread, |workspace, input| {
            workspace.retitle(input).await
        }
        TriageThread, |workspace, input| {
            workspace.triage(input).await
        }
        ConfigureThread, |workspace, ConfigureThread { thread, option, value }| {
            workspace.configure(thread, option, value).await?;
            Ok(Empty {})
        }
        DeleteThread, |workspace, input| {
            workspace.delete(input.thread, input.remove_worktree).await?;
            Ok(Empty {})
        }
        RemoveQueuedInput, |workspace, RemoveQueuedInput { thread, run }| {
            let mut db = workspace.db.lock().await;
            let mut tx = db.transaction().await?;
            store::require(&mut tx, thread).await?;
            runs::remove(&mut tx, thread, run).await?;
            tx.commit().await?;
            drop(db);
            workspace.settled(thread);
            Ok(Empty {})
        }
        UpdateQueuedInput, |workspace, input| {
            workspace.update_queue(input).await?;
            Ok(Empty {})
        }
        SteerQueuedInput, |workspace, input| {
            workspace.steer_queue(input).await?;
            Ok(Empty {})
        }
        AnswerRequest, |workspace, input| {
            workspace.answer(input, viz_protocol::AnsweredBy::User).await
        }
        SetThreadAccess, |workspace, SetThreadAccess { thread, access }| {
            workspace.set_thread_access(thread, access).await?;
            Ok(Empty {})
        }
        SetProjectAccess, |workspace, input| {
            workspace.set_project_access(input).await?;
            Ok(Empty {})
        }
        ListHarnesses, |workspace, _| {
            Ok(HarnessList { harnesses: workspace.list_harnesses().await? })
        }
        ListHarnessSessions, |workspace, input| {
            workspace.harness_sessions(input).await
        }
        ImportSession, |workspace, input| {
            workspace.import(input).await
        }
        HarnessOptions, |workspace, HarnessOptions { harness, model }| {
            let options = workspace.harness_options(&harness, Some(&model)).await?;
            Ok(HarnessOptionsResult { options })
        }
    }
}

impl Workspace {
    async fn update_queue(&self, input: UpdateQueuedInput) -> Result<(), Error> {
        let UpdateQueuedInput {
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

    async fn read_file_preview(&self, args: ReadFile) -> Result<FileContent, Error> {
        const TEXT_LIMIT: usize = 512 * 1024;
        const IMAGE_LIMIT: usize = 10 * 1024 * 1024;

        let requested = Path::new(&args.path);
        let resolved = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            let base = match (args.thread, args.project) {
                (Some(thread), _) => row(
                    &mut *self.db.lock().await,
                    "SELECT cwd FROM threads WHERE id = ?1",
                    [thread.into()],
                )
                .await?
                .ok_or(Error::NoThread(thread))?
                .str(0)?,
                (None, Some(project)) => {
                    store::preferences(&mut *self.db.lock().await, &project).await?;
                    project
                }
                (None, None) => {
                    return Err(Error::invalid(
                        "relative file path requires a thread or project",
                    ));
                }
            };
            Path::new(&base).join(requested)
        };

        let meta = tokio::fs::metadata(&resolved)
            .await
            .map_err(|error| Error::invalid(format!("couldn't read file: {error}")))?;
        if !meta.is_file() {
            return Err(Error::invalid("path is not a regular file"));
        }
        let size = meta.len();
        let guessed = mime_guess::from_path(&resolved)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let is_image = guessed.starts_with("image/");
        let limit = if is_image { IMAGE_LIMIT } else { TEXT_LIMIT };

        let file = tokio::fs::File::open(&resolved)
            .await
            .map_err(|error| Error::invalid(format!("couldn't open file: {error}")))?;
        let cap = usize::try_from(size).unwrap_or(limit).min(limit + 1);
        let mut buf = Vec::with_capacity(cap);
        file.take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
            .read_to_end(&mut buf)
            .await
            .map_err(|error| Error::invalid(format!("couldn't read file: {error}")))?;
        let truncated = buf.len() > limit;
        if truncated {
            buf.truncate(limit);
        }

        let path_str = resolved.display().to_string();
        if is_image {
            if truncated {
                return Ok(FileContent {
                    path: path_str,
                    mime: guessed.to_owned(),
                    text: None,
                    data: None,
                    truncated: true,
                    size,
                });
            }
            return Ok(FileContent {
                path: path_str,
                mime: guessed.to_owned(),
                text: None,
                data: Some(STANDARD.encode(&buf)),
                truncated: false,
                size,
            });
        }

        if let Ok(text) = String::from_utf8(buf) {
            let mime = if guessed == "application/octet-stream" {
                "text/plain"
            } else {
                guessed
            };
            return Ok(FileContent {
                path: path_str,
                mime: mime.to_owned(),
                text: Some(text),
                data: None,
                truncated,
                size,
            });
        }

        Ok(FileContent {
            path: path_str,
            mime: guessed.to_owned(),
            text: None,
            data: None,
            truncated,
            size,
        })
    }
}

fn page_limit(limit: Option<u16>, default: usize) -> Result<usize, Error> {
    match limit {
        None => Ok(default),
        Some(limit @ 1..=200) => Ok(limit.into()),
        Some(_) => Err(Error::invalid("limit must be between 1 and 200")),
    }
}
