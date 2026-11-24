//! Public workspace methods bind deserialized parameters to typed operations.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use viz_fs::{FsError, LineWindowSpec, RelPath, Root, sniff_mime};
use viz_protocol::{
    AnswerRequest, BranchList, BrowseFolders, CancelClone, ConfigureThread, CreateFolder,
    CreateProject, DeleteThread, Empty, FileBody, FileContent, FileList, FileRoot, ForkThread,
    GitActionRequest, GitDiffRequest, GitHubAuth, GitHubChecks, GitHubCreatePull, GitHubLaunch,
    GitHubPullPage, GitHubRepositories, GitHubSearch, GitStatusRequest, HarnessList,
    HarnessOptions, HarnessOptionsResult, ImportSession, InitRepository, LaunchThread,
    ListBranches, ListEntries, ListHarnessSessions, ListHarnesses, ListProjects, ListThreads,
    ListWorktrees, MoveThread, ProjectList, ProjectResult, PromoteThread, ReadClone, ReadFile,
    ReadProject, ReadThreadState, RemoveProject, RemoveQueuedInput, RemoveWorktree,
    RestoreCheckpoint, RetitleThread, SearchFiles, SearchHits, SearchThreads, SendInput,
    SetProjectAccess, SetThreadAccess, StartClone, StatBlob, SteerQueuedInput, StopThread,
    ThreadList, ThreadStateResult, TriageThread, UpdateProject, UpdateQueuedInput, UpdateThread,
    WaitThread,
};

use super::{Workspace, runs, store};
use crate::{error::Error, rpc::handlers};

handlers! {
    ROUTES for Arc<Workspace> {
        ListThreads, |workspace, ListThreads { project, archived }| {
            let rev = workspace.revision();
            let threads = workspace.list_threads(archived, project.as_deref()).await?;
            Ok(ThreadList { rev, threads })
        }
        ListEntries, |workspace, ListEntries { thread, anchor, limit }| {
            workspace.entries(thread, anchor, page_limit(limit, 500)?).await
        }
        ReadThreadState, |workspace, ReadThreadState { thread }| {
            Ok(ThreadStateResult { state: workspace.state(thread).await? })
        }
        SearchThreads, |workspace, SearchThreads { query, limit }| {
            let limit = page_limit(limit, 50)?;
            let hits = workspace
                .db
                .read(move |conn| store::search(conn, &query, limit))
                .await?;
            Ok(SearchHits { hits })
        }
        SearchFiles, |workspace, SearchFiles { thread, project, query, limit }| {
            let cwd = match (thread, project) {
                (Some(thread), None) => {
                    workspace.db.read(move |conn| store::thread_cwd(conn, thread)).await?
                }
                (None, Some(project)) => {
                    workspace.db.read(move |conn| store::require_project(conn, &project)).await?
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
        StatBlob, |workspace, StatBlob { thread, id }| {
            workspace.stat_blob(thread, &id).await
        }
        ListProjects, |workspace, _| {
            let rev = workspace.revision();
            let projects = workspace.list_projects().await?;
            Ok(ProjectList { rev, projects })
        }
        ReadProject, |workspace, ReadProject { project }| {
            let project = workspace.read_project(&project).await?;
            Ok(ProjectResult { project })
        }
        CreateProject, |workspace, CreateProject { path, name }| {
            Ok(ProjectResult { project: workspace.create_project(&path, name).await? })
        }
        UpdateProject, |workspace, ctx, UpdateProject { project, name, changes }| {
            workspace
                .update_project(&ctx.principal.key(), &project, name, changes.unwrap_or_default())
                .await?;
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
        GitHubLaunch, |workspace, ctx, args| { workspace.github_launch(ctx.principal.key(), args).await }
        GitHubRepositories, |workspace, args| { workspace.github_repositories(args).await }
        PromoteThread, |workspace, args| { workspace.promote_thread(args).await }
        RemoveProject, |workspace, RemoveProject { project }| {
            workspace.remove_project(&project).await?;
            Ok(Empty {})
        }
        ListBranches, |workspace, ListBranches { project, query, offset, limit }| {
            let project = workspace
                .db
                .read(move |conn| store::require_project(conn, &project))
                .await?;
            let query = query.unwrap_or_default().to_lowercase();
            let offset = offset.unwrap_or_default();
            let limit = page_limit(limit, 100)?;
            let repo = viz_git::Repository::discover(Path::new(&project), &workspace.closing).await?;
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
        LaunchThread, |workspace, ctx, input| {
            workspace.launch(ctx.principal.key(), input).await
        }
        ForkThread, |workspace, input| {
            workspace.fork(input).await
        }
        SendInput, |workspace, ctx, input| {
            if let Some(origin) = &input.origin {
                match ctx.connector() {
                    Some(name) if origin.connector == name => {}
                    Some(_) => {
                        return Err(Error::invalid(
                            "origin must belong to the connected connector",
                        ));
                    }
                    None => {
                        return Err(Error::invalid("origins require a connector WebSocket"));
                    }
                }
            }
            workspace.send(ctx.principal.key(), input).await
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
            workspace
                .db
                .write_tx(move |tx| {
                    store::require(tx, thread)?;
                    runs::remove(tx, thread, run)
                })
                .await?;
            workspace.settled(thread);
            Ok(Empty {})
        }
        UpdateQueuedInput, |workspace, ctx, input| {
            workspace.update_queue(ctx.principal.key(), input).await?;
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
    async fn update_queue(
        &self,
        principal_key: String,
        input: UpdateQueuedInput,
    ) -> Result<(), Error> {
        let UpdateQueuedInput {
            thread,
            run,
            before,
            prompt,
        } = input;
        let leases = Arc::clone(&self.leases);
        self.db
            .write_tx(move |tx| {
                store::require(tx, thread)?;
                if let Some(prompt) = &prompt {
                    super::attachments::claim_prompt(tx, &leases, &principal_key, thread, prompt)?;
                }
                runs::update_queue(tx, thread, run, prompt.as_ref(), before)
            })
            .await?;
        self.changed(Some(thread));
        Ok(())
    }

    async fn steer_queue(self: &Arc<Self>, input: SteerQueuedInput) -> Result<(), Error> {
        let thread = input.thread;
        let run = input.run;
        let ws = Arc::clone(self);
        self.db
            .write(move |conn| {
                let tx = super::sql::immediate(conn)?;
                store::require(&tx, thread)?;
                let active = runs::active(&tx, thread)?;
                runs::promote(&tx, thread, run)?;
                if let Some(active) = active {
                    store::withdraw_requests(&tx, Some(active), viz_protocol::Withdrawal::Steered)?;
                }
                tx.commit()?;
                if let Some(active) = active {
                    ws.steer_active(thread, active);
                }
                ws.kick(thread, false);
                Ok(())
            })
            .await?;
        self.changed(Some(thread));
        Ok(())
    }

    async fn read_file_preview(&self, args: ReadFile) -> Result<FileContent, Error> {
        const TEXT_LIMIT: usize = viz_protocol::limits::PREVIEW_TEXT_BYTES;
        const IMAGE_LIMIT: usize = viz_protocol::limits::PREVIEW_IMAGE_BYTES;
        const DEFAULT_WINDOW_LINES: u32 = 600;
        const MAX_WINDOW_LINES: u32 = 2_000;

        let rel = RelPath::parse(&args.path).map_err(|error| Error::invalid(error.to_string()))?;
        let root_dir = match args.root {
            FileRoot::Thread { thread } => {
                self.db
                    .read(move |conn| store::thread_cwd(conn, thread))
                    .await?
            }
            FileRoot::Project { project } => {
                self.db
                    .read(move |conn| store::require_project(conn, &project))
                    .await?
            }
        };

        let spec = args.window.map_or(
            LineWindowSpec {
                start: 1,
                count: DEFAULT_WINDOW_LINES,
            },
            |window| LineWindowSpec {
                start: window.start.max(1),
                count: window.count.clamp(1, MAX_WINDOW_LINES),
            },
        );
        let root_path = PathBuf::from(root_dir);
        tokio::task::spawn_blocking(move || {
            let root = Root::open(&root_path).map_err(|error| {
                Error::invalid(format!("couldn't open workspace root: {error}"))
            })?;
            let file = root
                .open_regular(&rel)
                .map_err(|error| Error::invalid(format!("couldn't open file: {error}")))?;
            let size = file.len();
            let path_str = rel.to_string();
            let guessed = mime_guess::from_path(rel.as_path())
                .first_raw()
                .unwrap_or("application/octet-stream");
            if guessed.starts_with("image/") {
                let (header, total) = file
                    .read_range(0, 4096)
                    .map_err(|error| Error::invalid(format!("couldn't inspect image: {error}")))?;
                let mime = sniff_mime(rel.as_path(), &header);
                let body = if total > IMAGE_LIMIT as u64 {
                    FileBody::Binary { too_large: true }
                } else {
                    FileBody::Image {}
                };
                return Ok(FileContent {
                    path: path_str,
                    mime,
                    size: total,
                    body,
                });
            }
            match file.read_line_window(spec, TEXT_LIMIT) {
                Ok(window) => {
                    let mime = if guessed == "application/octet-stream" {
                        "text/plain"
                    } else {
                        guessed
                    };
                    Ok(FileContent {
                        path: path_str,
                        mime: mime.to_owned(),
                        size: window.size,
                        body: FileBody::Text {
                            text: window.text,
                            start_line: window.start_line,
                            line_count: window.line_count,
                            total_lines: window.total_lines,
                            truncated: window.truncated,
                            has_more: window.has_more,
                        },
                    })
                }
                Err(FsError::InvalidUtf8) => Ok(FileContent {
                    path: path_str,
                    mime: guessed.to_owned(),
                    size,
                    body: FileBody::Binary { too_large: false },
                }),
                Err(error) => Err(Error::invalid(format!("couldn't read file: {error}"))),
            }
        })
        .await
        .map_err(|error| Error::unavailable(format!("file preview worker failed: {error}")))?
    }
}

fn page_limit(limit: Option<u16>, default: usize) -> Result<usize, Error> {
    match limit {
        None => Ok(default),
        Some(limit @ 1..=200) => Ok(limit.into()),
        Some(_) => Err(Error::invalid("limit must be between 1 and 200")),
    }
}
